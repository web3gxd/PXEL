use super::*;
use spreadsheet_application::journal::OperationJournal;
use spreadsheet_domain::workbook::{
    ColumnId, CommittedEvent, EventPrincipal, Operation, OperationId, RowId, SheetId, SheetSeed,
    Workbook,
};
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const PHC: &str = "$argon2id$v=19$m=65536,t=3,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
fn secret() -> Result<ShareSecretPhc, ShareError> {
    ShareSecretPhc::new(ShareSecretKind::Pin, Zeroizing::new(PHC.to_owned()))
}
fn request(value: u8) -> Uuid {
    Uuid::from_bytes([value; 16])
}

async fn fixture(
    pool: sqlx::PgPool,
) -> Result<(PgDatabase, AuthenticatedActor, DocumentId), Box<dyn std::error::Error>> {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let owner = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    };
    db.issue_development_session([9; 32], owner.actor_id, owner.workspace_id)
        .await?;
    let document = DocumentId::from_bytes([3; 16])?;
    let operation = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: SheetId::from_bytes([20; 16])?,
            name: "Protected share fixture".into(),
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
    Ok((db, owner, document))
}
async fn link(
    db: &PgDatabase,
    owner: AuthenticatedActor,
    document: DocumentId,
    value: u8,
    role: ShareRole,
) -> Result<(Uuid, [u8; 32]), ShareError> {
    let receipt = db
        .create_share(
            owner,
            document,
            request(value),
            role,
            ShareLifetime::Hour,
            secret()?,
            [7; 32],
        )
        .await?;
    let token = receipt.token.as_ref().ok_or(ShareError::Corrupt)?;
    Ok((
        receipt.link.link_id,
        Sha256::digest(token.as_slice()).into(),
    ))
}
fn seed(value: u8) -> Result<GuestSessionSeed, ShareError> {
    Ok(GuestSessionSeed {
        session_id: request(value),
        actor_id: ActorId::from_bytes([value; 16]).map_err(|_| ShareError::InvalidInput)?,
        token_hash: [value; 32],
    })
}
async fn login(
    db: &PgDatabase,
    link: Uuid,
    hash: [u8; 32],
    value: u8,
) -> Result<GuestSession, ShareError> {
    let attempt = db.begin_share_attempt(link, hash).await?;
    if attempt.phc() != Some(PHC) {
        return Err(ShareError::Corrupt);
    }
    match attempt.complete(true, Some(seed(value)?)).await? {
        ShareAttemptOutcome::Accepted(session) => Ok(session),
        _ => Err(ShareError::Corrupt),
    }
}
async fn fail(
    db: &PgDatabase,
    link: Uuid,
    hash: [u8; 32],
) -> Result<ShareAttemptOutcome, ShareError> {
    db.begin_share_attempt(link, hash)
        .await?
        .complete(false, None)
        .await
}
async fn finish_block(db: &PgDatabase, link: Uuid) -> Result<(), ShareError> {
    sqlx::query("UPDATE share_links SET blocked_until=clock_timestamp()-interval '1 second' WHERE link_id=$1 AND status='active'").bind(link).execute(&db.pool).await?;
    Ok(())
}
fn rejected(result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>) -> bool {
    matches!(result,Err(sqlx::Error::Database(error)) if matches!(error.code().as_deref(),Some("23514"|"55000"|"23503")))
}

#[test]
fn phc_profile_is_bounded_and_exact() -> TestResult {
    secret()?;
    for text in [
        PHC.replace("argon2id", "argon2i"),
        PHC.replace("v=19", "v=16"),
        PHC.replace("m=65536", "m=1024"),
        PHC.replace("t=3", "t=2"),
        PHC.replace("p=1", "p=2"),
        PHC.replace("$AAAAAAAAAAAAAAAAAAAAAA$", "$AAAAAAAAAAAAAA$"),
        format!("{PHC}$extra"),
        PHC.replacen("AAAAA", "=AAAA", 1),
    ] {
        assert!(matches!(
            ShareSecretPhc::new(ShareSecretKind::Password, Zeroizing::new(text)),
            Err(ShareError::InvalidInput)
        ));
    }
    assert!(matches!(
        ShareSecretPhc::new(ShareSecretKind::Pin, Zeroizing::new("x".repeat(257))),
        Err(ShareError::InvalidInput)
    ));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn tokens_and_phc_are_protected_and_create_replay_never_returns_token(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let first = db
        .create_share(
            owner,
            document,
            request(60),
            ShareRole::Edit,
            ShareLifetime::Hour,
            secret()?,
            [7; 32],
        )
        .await?;
    assert!(first.newly_committed);
    let token = first.token.as_ref().ok_or("initial token absent")?;
    let (stored_hash, stored_phc, stored_intent) =
        sqlx::query_as::<_, (Vec<u8>, Vec<u8>, Vec<u8>)>(
            "SELECT token_hash,secret_phc,secret_intent FROM share_links WHERE link_id=$1",
        )
        .bind(first.link.link_id)
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(stored_hash, Sha256::digest(token.as_slice()).as_slice());
    assert!(
        !stored_phc
            .windows(PHC.len())
            .any(|bytes| bytes == PHC.as_bytes())
    );
    assert_eq!(
        &**db
            .open_record(
                document,
                1,
                "share-secret",
                first.link.link_id,
                1,
                &stored_phc,
                MAX_PHC_BYTES
            )
            .await?,
        PHC.as_bytes()
    );
    assert_eq!(
        &**db
            .open_record(
                document,
                1,
                "share-intent",
                first.link.link_id,
                1,
                &stored_intent,
                32
            )
            .await?,
        &[7_u8; 32]
    );
    assert!(
        db.open_record(
            document,
            1,
            "share-secret",
            request(99),
            1,
            &stored_phc,
            MAX_PHC_BYTES
        )
        .await
        .is_err()
    );
    let reservations =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?;
    let salted = ShareSecretPhc::new(
        ShareSecretKind::Pin,
        Zeroizing::new(PHC.replace("$AAAAAAAAAAAAAAAAAAAAAA$", "$BBBBBBBBBBBBBBBBBBBBBA$")),
    )?;
    let replay = db
        .create_share(
            owner,
            document,
            request(60),
            ShareRole::Edit,
            ShareLifetime::Hour,
            salted,
            [7; 32],
        )
        .await?;
    assert_eq!(replay.link.link_id, first.link.link_id);
    assert!(replay.token.is_none());
    assert!(!replay.newly_committed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?,
        reservations
    );
    assert!(matches!(
        db.create_share(
            owner,
            document,
            request(60),
            ShareRole::Edit,
            ShareLifetime::Hour,
            secret()?,
            [8; 32]
        )
        .await,
        Err(ShareError::IdempotencyConflict)
    ));
    assert!(matches!(
        db.create_share(
            owner,
            document,
            request(60),
            ShareRole::View,
            ShareLifetime::Hour,
            secret()?,
            [7; 32]
        )
        .await,
        Err(ShareError::IdempotencyConflict)
    ));
    assert!(matches!(
        db.create_share(
            owner,
            document,
            request(61),
            ShareRole::View,
            ShareLifetime::Indefinite { confirmed: false },
            secret()?,
            [7; 32]
        )
        .await,
        Err(ShareError::InvalidInput)
    ));
    let revoked = db
        .revoke_share(owner, document, first.link.link_id, request(62))
        .await?;
    assert!(revoked.newly_committed);
    let revoked_retry = db
        .revoke_share(owner, document, first.link.link_id, request(62))
        .await?;
    assert!(!revoked_retry.newly_committed);
    assert_eq!(revoked_retry.access_epoch, revoked.access_epoch);
    let replay = db
        .create_share(
            owner,
            document,
            request(60),
            ShareRole::Edit,
            ShareLifetime::Hour,
            secret()?,
            [7; 32],
        )
        .await?;
    assert!(replay.token.is_none());
    assert_eq!(replay.link.status, ShareStatus::Revoked);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn unknown_and_wrong_tokens_do_not_expose_phc_or_increment_counters(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::View).await?;
    for target in [request(98), link] {
        let attempt = db.begin_share_attempt(target, [0; 32]).await?;
        assert!(attempt.phc().is_none());
        assert!(matches!(
            attempt.complete(false, None).await?,
            ShareAttemptOutcome::Invalid
        ));
    }
    let summary = db.list_shares(owner, document, 100).await?;
    assert_eq!(summary[0].failed_attempts, 0);
    assert_eq!(summary[0].lockouts, 0);
    assert!(matches!(
        fail(&db, link, hash).await?,
        ShareAttemptOutcome::Invalid
    ));
    assert_eq!(
        db.list_shares(owner, document, 100).await?[0].failed_attempts,
        1
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn concurrent_failures_commit_exactly_one_fifth_failure_lock(
    pool: sqlx::PgPool,
) -> TestResult {
    let (mut db, owner, document) = fixture(pool).await?;
    db.crypto_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with((*db.pool.connect_options()).clone())
        .await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::Edit).await?;
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(5));
    let mut tasks = Vec::new();
    for _ in 0..5 {
        let db = db.clone();
        let start = start.clone();
        tasks.push(tokio::spawn(async move {
            start.wait().await;
            for _ in 0..100 {
                match fail(&db, link, hash).await {
                    Err(ShareError::Conflict) => tokio::time::sleep(Duration::from_millis(5)).await,
                    result => return result,
                }
            }
            Err(ShareError::Conflict)
        }));
    }
    let mut blocked = 0;
    for task in tasks {
        if matches!(task.await??, ShareAttemptOutcome::Blocked { .. }) {
            blocked += 1;
        }
    }
    assert_eq!(blocked, 1);
    let summary = db.list_shares(owner, document, 100).await?;
    assert_eq!(summary[0].failed_attempts, 0);
    assert_eq!(summary[0].lockouts, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM share_audit_events WHERE event_type='authentication_failed'"
        )
        .fetch_one(&db.pool)
        .await?,
        5
    );
    let deadline = sqlx::query_scalar::<_, String>(
        "SELECT blocked_until::text FROM share_links WHERE link_id=$1",
    )
    .bind(link)
    .fetch_one(&db.pool)
    .await?;
    for _ in 0..3 {
        assert!(matches!(
            fail(&db, link, hash).await?,
            ShareAttemptOutcome::Blocked {
                remaining_seconds: 1..=900
            }
        ));
    }
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT blocked_until::text FROM share_links WHERE link_id=$1"
        )
        .bind(link)
        .fetch_one(&db.pool)
        .await?,
        deadline
    );
    db.crypto_pool.close().await;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn third_consecutive_lockout_survives_reopen_revokes_sessions_and_notifies_owner(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::Edit).await?;
    let guest = login(&db, link, hash, 70).await?;
    assert_eq!(guest.admitted_head_version, DocumentVersion::new(1)?);
    for round in 0..3 {
        for index in 0..5 {
            let result = fail(&db, link, hash).await?;
            if index == 4 && round < 2 {
                assert!(matches!(
                    result,
                    ShareAttemptOutcome::Blocked {
                        remaining_seconds: 900
                    }
                ));
            }
            if index == 4 && round == 2 {
                assert!(matches!(
                    result,
                    ShareAttemptOutcome::Invalidated(ShareMutationReceipt {
                        newly_committed: true,
                        ..
                    })
                ));
            }
        }
        if round < 2 {
            assert!(db.guest_session(link, [70; 32]).await?.is_some());
            finish_block(&db, link).await?;
        }
        let reopened = PgDatabase {
            pool: db.pool.clone(),
            crypto_pool: db.crypto_pool.clone(),
            keys: db.keys.clone(),
        };
        assert_eq!(
            reopened.list_shares(owner, document, 100).await?[0].lockouts,
            round + 1
        );
    }
    assert!(db.guest_session(link, [70; 32]).await?.is_none());
    let notifications = db.owner_notifications(owner, 100).await?;
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].reason, "disabled");
    assert_eq!(
        db.list_shares(owner, document, 100).await?[0].status,
        ShareStatus::Disabled
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT access_epoch FROM documents WHERE document_id=$1")
            .bind(id(document.as_bytes()))
            .fetch_one(&db.pool)
            .await?,
        1
    );
    assert!(rejected(
        sqlx::query("UPDATE share_audit_events SET event_type='created'")
            .execute(&db.pool)
            .await
    ));
    assert!(rejected(
        sqlx::query("DELETE FROM share_owner_notifications")
            .execute(&db.pool)
            .await
    ));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn successful_admission_resets_failures_and_consecutive_locks_and_never_reuses_author(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::Edit).await?;
    for _ in 0..5 {
        fail(&db, link, hash).await?;
    }
    finish_block(&db, link).await?;
    let first = login(&db, link, hash, 70).await?;
    let summary = db.list_shares(owner, document, 100).await?;
    assert_eq!(summary[0].lockouts, 0);
    assert_eq!(summary[0].failed_attempts, 0);
    for _ in 0..4 {
        fail(&db, link, hash).await?;
    }
    let second = login(&db, link, hash, 71).await?;
    assert_ne!(first.identity.actor_id, second.identity.actor_id);
    assert_eq!(
        db.list_shares(owner, document, 100).await?[0].failed_attempts,
        0
    );
    let attempt = db.begin_share_attempt(link, hash).await?;
    assert!(matches!(
        attempt
            .complete(
                true,
                Some(GuestSessionSeed {
                    session_id: request(72),
                    actor_id: first.identity.actor_id,
                    token_hash: [72; 32]
                })
            )
            .await,
        Err(ShareError::Conflict)
    ));
    assert!(db.guest_session(link, [70; 32]).await?.is_some());
    assert!(db.guest_session(request(99), [70; 32]).await?.is_none());
    assert!(db.guest_session(link, hash).await?.is_none());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn pending_hash_holds_no_document_or_maintenance_lock_and_revocation_wins(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::Edit).await?;
    let attempt = db.begin_share_attempt(link, hash).await?;
    assert_eq!(attempt.phc(), Some(PHC));
    assert!(matches!(
        db.begin_share_attempt(link, hash).await,
        Err(ShareError::Conflict)
    ));
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock(1347962188)")
            .fetch_one(&db.pool)
            .await?
    );
    let revoked = tokio::time::timeout(
        Duration::from_secs(2),
        db.revoke_share(owner, document, link, request(61)),
    )
    .await??;
    assert!(revoked.newly_committed);
    assert!(matches!(
        attempt.complete(true, Some(seed(70)?)).await?,
        ShareAttemptOutcome::Invalid
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM share_guest_sessions")
            .fetch_one(&db.pool)
            .await?,
        0
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn guest_database_guards_reject_member_privilege_escalation_and_ancient_commands(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::Edit).await?;
    let guest = login(&db, link, hash, 70).await?;
    let guest_id = id(guest.identity.actor_id.as_bytes());
    let workspace = id(owner.workspace_id.as_bytes());
    assert!(rejected(
        sqlx::query("UPDATE users SET identity_kind='member' WHERE user_id=$1")
            .bind(guest_id)
            .execute(&db.pool)
            .await
    ));
    for statement in [
        "INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')",
        "UPDATE memberships SET user_id=$2 WHERE workspace_id=$1",
    ] {
        assert!(rejected(
            sqlx::query(statement)
                .bind(workspace)
                .bind(guest_id)
                .execute(&db.pool)
                .await
        ));
    }
    assert!(rejected(sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'editor')").bind(workspace).bind(id(document.as_bytes())).bind(guest_id).execute(&db.pool).await));
    assert!(rejected(
        sqlx::query("UPDATE document_permissions SET user_id=$1 WHERE document_id=$2")
            .bind(guest_id)
            .bind(id(document.as_bytes()))
            .execute(&db.pool)
            .await
    ));
    assert!(rejected(sqlx::query("INSERT INTO development_accounts(account,user_id,workspace_id) VALUES('guest-forbidden',$1,$2)").bind(guest_id).bind(workspace).execute(&db.pool).await));
    assert!(rejected(
        sqlx::query("UPDATE development_accounts SET user_id=$1")
            .bind(guest_id)
            .execute(&db.pool)
            .await
    ));
    assert!(rejected(
        sqlx::query(
            "INSERT INTO development_sessions(token_hash,user_id,workspace_id) VALUES($1,$2,$3)"
        )
        .bind([88_u8; 32].as_slice())
        .bind(guest_id)
        .bind(workspace)
        .execute(&db.pool)
        .await
    ));
    assert!(rejected(
        sqlx::query("UPDATE development_sessions SET user_id=$1")
            .bind(guest_id)
            .execute(&db.pool)
            .await
    ));
    assert!(rejected(
        sqlx::query("UPDATE share_guest_sessions SET admitted_head_version=0 WHERE user_id=$1")
            .bind(guest_id)
            .execute(&db.pool)
            .await
    ));
    let mut tx = db.begin_guarded().await?;
    let access = guest_access_locked(&mut tx, guest.identity, document).await?;
    assert!(access.can_edit);
    assert!(matches!(
        guest_command_floor_locked(&mut tx, guest.identity, document, DocumentVersion::ZERO).await,
        Err(JournalError::Unauthorized)
    ));
    guest_command_floor_locked(&mut tx, guest.identity, document, DocumentVersion::new(1)?).await?;
    tx.rollback().await?;
    assert!(rejected(sqlx::query("INSERT INTO document_operations(document_id,committed_version,operation_id,actor_id,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash,principal_kind) SELECT document_id,1,$1,$2,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash,'user' FROM document_operations LIMIT 1").bind(request(90)).bind(guest_id).execute(&db.pool).await));
    db.revoke_share(owner, document, link, request(61)).await?;
    let mut tx = db.begin_guarded().await?;
    assert!(matches!(
        guest_access_locked(&mut tx, guest.identity, document).await,
        Err(JournalError::Unauthorized)
    ));
    tx.rollback().await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn view_guests_have_no_edit_capability_and_other_documents_are_denied(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::View).await?;
    let guest = login(&db, link, hash, 70).await?;
    let mut tx = db.begin_guarded().await?;
    assert!(
        !guest_access_locked(&mut tx, guest.identity, document)
            .await?
            .can_edit
    );
    assert!(matches!(
        guest_access_locked(&mut tx, guest.identity, DocumentId::from_bytes([4; 16])?).await,
        Err(JournalError::Unauthorized) | Err(JournalError::NotFound)
    ));
    tx.rollback().await?;
    assert!(rejected(sqlx::query("INSERT INTO document_operations(document_id,committed_version,operation_id,actor_id,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash,principal_kind) SELECT document_id,2,$1,$2,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash,'user' FROM document_operations LIMIT 1").bind(request(90)).bind(id(guest.identity.actor_id.as_bytes())).execute(&db.pool).await));
    assert!(matches!(
        db.list_shares(guest.identity, document, 100).await,
        Err(ShareError::Unauthorized)
    ));
    assert!(
        db.owner_notifications(guest.identity, 100)
            .await?
            .is_empty()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn expiry_after_hash_is_checked_on_completion_and_durably_revokes_guest(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let link = request(60);
    let phc = db
        .seal_record(
            document,
            1,
            "share-secret",
            link,
            1,
            PHC.as_bytes(),
            MAX_PHC_BYTES,
        )
        .await?;
    let intent = db
        .seal_record(document, 1, "share-intent", link, 1, &[7; 32], 32)
        .await?;
    sqlx::query("WITH t AS (SELECT clock_timestamp() AS now) INSERT INTO share_links(link_id,workspace_id,document_id,owner_id,create_request_id,token_hash,role,secret_kind,secret_phc,secret_intent,created_at,expires_at) SELECT $1,$2,$3,$4,$5,$6,'edit','pin',$7,$8,t.now,t.now+interval '1 second' FROM t")
        .bind(link).bind(id(owner.workspace_id.as_bytes())).bind(id(document.as_bytes())).bind(id(owner.actor_id.as_bytes())).bind(request(61)).bind([8_u8;32].as_slice()).bind(phc).bind(intent).execute(&db.pool).await?;
    login(&db, link, [8; 32], 70).await?;
    let attempt = db.begin_share_attempt(link, [8; 32]).await?;
    assert!(attempt.phc().is_some());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(db.expired_share_documents(100).await?, vec![document]);
    assert!(matches!(
        attempt.complete(false, None).await?,
        ShareAttemptOutcome::Invalidated(ShareMutationReceipt {
            access_epoch: 1,
            ..
        })
    ));
    assert!(db.guest_session(link, [70; 32]).await?.is_none());
    assert!(db.expire_document_shares(document).await?.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM share_guest_sessions WHERE revoked_at IS NOT NULL"
        )
        .fetch_one(&db.pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM share_audit_events WHERE event_type='expired'"
        )
        .fetch_one(&db.pool)
        .await?,
        1
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn access_epoch_notifications_reach_independent_listeners_after_commit(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, _owner, document) = fixture(pool).await?;
    let mut first = db.listen_access_epochs().await?;
    let mut second = db.listen_access_epochs().await?;
    let mut tx = db.pool.begin().await?;
    sqlx::query("UPDATE documents SET access_epoch=access_epoch+1 WHERE document_id=$1")
        .bind(id(document.as_bytes()))
        .execute(&mut *tx)
        .await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), first.recv())
            .await
            .is_err()
    );
    tx.commit().await?;
    for listener in [&mut first, &mut second] {
        let notification = tokio::time::timeout(Duration::from_secs(2), listener.recv()).await??;
        assert_eq!(notification.document, document);
        assert_eq!(notification.epoch, 1);
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn delivery_access_lease_fences_share_revocation_across_instances(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::Edit).await?;
    let guest = login(&db, link, hash, 70).await?;
    let lease = db.lease_delivery_access(guest.identity, document).await?;
    assert!(lease.access().can_edit);

    // A second database handle models a different serving process. Its durable
    // revocation cannot commit until the in-flight bounded payload releases the
    // shared PostgreSQL authorization lock.
    let remote = db.clone();
    let revoke = tokio::spawn(async move {
        remote
            .revoke_share(owner, document, link, request(61))
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!revoke.is_finished());
    lease.release().await?;
    assert!(revoke.await??.newly_committed);
    assert!(db.guest_session(link, [70; 32]).await?.is_none());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn restore_cleanup_revokes_links_sessions_and_retains_authors(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, document) = fixture(pool).await?;
    let (link, hash) = link(&db, owner, document, 60, ShareRole::Edit).await?;
    let guest = login(&db, link, hash, 70).await?;
    let mut tx = db.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1347962188)")
        .execute(&mut *tx)
        .await?;
    assert_eq!(
        invalidate_document_shares_after_restore(&mut tx, document).await?,
        Some(1)
    );
    tx.commit().await?;
    assert!(db.guest_session(link, [70; 32]).await?.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT identity_kind FROM users WHERE user_id=$1")
            .bind(id(guest.identity.actor_id.as_bytes()))
            .fetch_one(&db.pool)
            .await?,
        "share_guest"
    );
    assert_eq!(
        db.list_shares(owner, document, 100).await?[0].status,
        ShareStatus::Revoked
    );
    let mut tx = db.pool.begin().await?;
    assert!(
        invalidate_document_shares_after_restore(&mut tx, document)
            .await?
            .is_none()
    );
    tx.rollback().await?;
    Ok(())
}
