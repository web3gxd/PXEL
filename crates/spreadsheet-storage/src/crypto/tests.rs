use super::*;
use spreadsheet_application::keys::KeyProvider;
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const KEYRING: &str = r#"{"schema_version":1,"active_kek_id":"kek-a","keys":[{"id":"kek-a","key_hex":"0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20"},{"id":"alias","key_hex":"0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20"}]}"#;

fn provider() -> Result<LocalKeyProvider, KeyProviderError> {
    LocalKeyProvider::from_keyring_json(KEYRING)
}

fn document(value: u8) -> Result<DocumentId, spreadsheet_domain::workbook::DomainError> {
    DocumentId::from_bytes([value; 16])
}

fn workspace(value: u8) -> Result<WorkspaceId, spreadsheet_domain::workbook::DomainError> {
    WorkspaceId::from_bytes([value; 16])
}

fn test_key() -> Result<DocumentKey, spreadsheet_domain::workbook::DomainError> {
    Ok(DocumentKey {
        workspace: workspace(1)?,
        generation: 1,
        physical_id: physical_key_id(&[8; 32]),
        material: Zeroizing::new([8; 32]),
    })
}

#[test]
fn local_provider_rejects_invalid_keyrings_without_exposing_input() {
    for input in [
        "",
        "not-json-secret",
        r#"{"schema_version":2,"active_kek_id":"a","keys":[]}"#,
        r#"{"schema_version":1,"active_kek_id":"missing","keys":[{"id":"a","key_hex":"secret"}]}"#,
        r#"{"schema_version":1,"active_kek_id":"a","keys":[{"id":"a","key_hex":"0000000000000000000000000000000000000000000000000000000000000000"}]}"#,
        r#"{"schema_version":1,"active_kek_id":"a","keys":[{"id":"a","key_hex":"0101010101010101010101010101010101010101010101010101010101010101"},{"id":"a","key_hex":"0101010101010101010101010101010101010101010101010101010101010101"}]}"#,
        r#"{"schema_version":1,"active_kek_id":"a","keys":[],"extra":"secret"}"#,
        r#"{"schema_version":1,"active_kek_id":"a","keys":[{"id":"a","key_hex":"\u003001010101010101010101010101010101010101010101010101010101010101"}]}"#,
    ] {
        let result = LocalKeyProvider::from_keyring_json(input);
        assert!(matches!(
            result,
            Err(KeyProviderError::InvalidConfiguration)
        ));
        if let Err(error) = result {
            assert!(!error.to_string().contains("secret"));
        }
    }
    assert!(LocalKeyProvider::from_keyring_json(&"x".repeat(65537)).is_err());
}

#[test]
fn configuration_sources_are_exclusive_and_never_fall_back() -> TestResult {
    assert_eq!(
        LocalKeyProvider::from_sources(Some(KEYRING.into()), None)?.active_kek_id(),
        "kek-a"
    );
    for (inline, file) in [
        (None, None),
        (Some(KEYRING.into()), Some("missing-keyring.json".into())),
        (Some("".into()), None),
        (None, Some("".into())),
        (None, Some("missing-keyring.json".into())),
    ] {
        assert!(matches!(
            LocalKeyProvider::from_sources(inline, file),
            Err(KeyProviderError::InvalidConfiguration)
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_wide(&[0xd800]);
        assert!(matches!(
            LocalKeyProvider::from_sources(Some(invalid), None),
            Err(KeyProviderError::InvalidConfiguration)
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_vec(vec![0xff]);
        assert!(matches!(
            LocalKeyProvider::from_sources(Some(invalid), None),
            Err(KeyProviderError::InvalidConfiguration)
        ));
    }
    Ok(())
}

#[test]
fn private_keyring_file_is_required_and_bounded() -> TestResult {
    let local = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .ok_or("workspace root is unavailable")?
        .join(".local");
    std::fs::create_dir_all(&local)?;
    let directory = tempfile::tempdir_in(local)?;
    let path = directory.path().join("test-keyring.json");
    std::fs::write(&path, KEYRING)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        let script = r#"$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=Get-Acl -LiteralPath $env:PXEL_TEST_KEY_FILE; $acl.SetAccessRuleProtection($true,$false); $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','Allow'); $acl.AddAccessRule($rule); Set-Acl -LiteralPath $env:PXEL_TEST_KEY_FILE -AclObject $acl"#;
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env_remove("PSModulePath")
            .env("PXEL_TEST_KEY_FILE", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        assert!(status.success());
    }
    assert_eq!(LocalKeyProvider::from_file(&path)?.active_kek_id(), "kek-a");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
    }
    #[cfg(windows)]
    {
        // Change only the DACL. Rewriting a fetched full security descriptor
        // can request SACL privileges unrelated to this read-permission case.
        let status = std::process::Command::new("icacls.exe")
            .arg(&path)
            .args(["/grant", "*S-1-5-32-545:R"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        assert!(status.success());
    }
    assert!(matches!(
        LocalKeyProvider::from_file(&path),
        Err(KeyProviderError::InvalidConfiguration)
    ));
    std::fs::write(&path, "x".repeat(65537))?;
    assert!(matches!(
        LocalKeyProvider::from_file(&path),
        Err(KeyProviderError::InvalidConfiguration)
    ));
    assert!(matches!(
        LocalKeyProvider::from_file(directory.path()),
        Err(KeyProviderError::InvalidConfiguration)
    ));
    Ok(())
}

#[test]
fn wrapping_authenticates_context_and_keeps_alias_physical_identity() -> TestResult {
    let provider = provider()?;
    assert_eq!(provider.active_kek_id(), "kek-a");
    assert_eq!(
        provider.physical_key_id("kek-a")?,
        provider.physical_key_id("alias")?
    );
    let wrapped = provider.wrap_key(
        "kek-a",
        &[4; 24],
        b"workspace-document-generation",
        &[7; 32],
    )?;
    assert_eq!(wrapped.len(), 48);
    assert_eq!(
        *provider.unwrap_key(
            "alias",
            &[4; 24],
            b"workspace-document-generation",
            &wrapped
        )?,
        [7; 32]
    );
    assert!(matches!(
        provider.unwrap_key("kek-a", &[4; 24], b"wrong-document", &wrapped),
        Err(KeyProviderError::AuthenticationFailed)
    ));
    assert!(matches!(
        provider.unwrap_key(
            "missing",
            &[4; 24],
            b"workspace-document-generation",
            &wrapped
        ),
        Err(KeyProviderError::KeyUnavailable)
    ));
    let mut corrupt = wrapped;
    corrupt[47] ^= 1;
    assert!(
        provider
            .unwrap_key(
                "kek-a",
                &[4; 24],
                b"workspace-document-generation",
                &corrupt
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn authenticated_envelope_rejects_each_context_substitution() -> TestResult {
    let mut key = test_key()?;
    let doc = document(2)?;
    let object = Uuid::from_bytes([3; 16]);
    let aad = record_aad(&key, doc, 7, "journal", object, 3)?;
    let envelope = seal_with(&key, &[9; 24], &aad, b"confidential formula and value")?;
    let (generation, nonce) = parse_envelope(&envelope, 64)?;
    assert_eq!(generation, 1);
    assert_eq!(
        &**open_with(&key, &nonce, &aad, &envelope)?,
        b"confidential formula and value"
    );
    for substituted in [
        record_aad(&key, document(4)?, 7, "journal", object, 3)?,
        record_aad(&key, doc, 8, "journal", object, 3)?,
        record_aad(&key, doc, 7, "snapshot", object, 3)?,
        record_aad(&key, doc, 7, "journal", Uuid::from_bytes([4; 16]), 3)?,
        record_aad(&key, doc, 7, "journal", object, 4)?,
    ] {
        assert!(matches!(
            open_with(&key, &nonce, &substituted, &envelope),
            Err(CryptoError::AuthenticationFailed)
        ));
    }
    key.workspace = workspace(5)?;
    assert!(
        open_with(
            &key,
            &nonce,
            &record_aad(&key, doc, 7, "journal", object, 3)?,
            &envelope
        )
        .is_err()
    );
    key.workspace = workspace(1)?;
    key.generation = 2;
    assert!(
        open_with(
            &key,
            &nonce,
            &record_aad(&key, doc, 7, "journal", object, 3)?,
            &envelope
        )
        .is_err()
    );
    key.generation = 1;
    key.material = Zeroizing::new([99; 32]);
    assert!(open_with(&key, &nonce, &aad, &envelope).is_err());
    Ok(())
}

#[test]
fn malformed_versions_tags_nonces_and_limits_fail_before_plaintext_publication() -> TestResult {
    let key = test_key()?;
    let aad = record_aad(
        &key,
        document(2)?,
        1,
        "journal",
        Uuid::from_bytes([3; 16]),
        1,
    )?;
    let envelope = seal_with(&key, &[9; 24], &aad, b"abcd")?;
    assert_eq!(envelope.len(), 4 + CRYPTO_ENVELOPE_OVERHEAD);
    assert_eq!(ciphertext_limit(4)?, envelope.len());
    assert!(ciphertext_limit(usize::MAX).is_err());
    for length in 0..CRYPTO_ENVELOPE_OVERHEAD {
        assert!(parse_envelope(&envelope[..length], 64).is_err());
    }
    assert!(matches!(
        parse_envelope(&envelope, 3),
        Err(CryptoError::LimitExceeded { limit: 3 })
    ));
    for position in [8, 10] {
        let mut unsupported = envelope.clone();
        unsupported[position] = 7;
        assert!(matches!(
            parse_envelope(&unsupported, 64),
            Err(CryptoError::UnsupportedVersion)
        ));
    }
    for position in [16, 40, 59] {
        let mut corrupt = envelope.clone();
        corrupt[position] ^= 1;
        let (_, nonce) = parse_envelope(&corrupt, 64)?;
        assert!(matches!(
            open_with(&key, &nonce, &aad, &corrupt),
            Err(CryptoError::AuthenticationFailed)
        ));
    }
    assert!(
        record_aad(
            &key,
            document(2)?,
            0,
            "journal",
            Uuid::from_bytes([3; 16]),
            1
        )
        .is_err()
    );
    assert!(
        record_aad(
            &key,
            document(2)?,
            1,
            "journal\0",
            Uuid::from_bytes([3; 16]),
            1
        )
        .is_err()
    );
    let empty = seal_with(&key, &[10; 24], &aad, b"")?;
    let (_, nonce) = parse_envelope(&empty, 0)?;
    assert!(open_with(&key, &nonce, &aad, &empty)?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn durable_nonces_retry_collisions_and_survive_aborted_writes(pool: PgPool) -> TestResult {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let physical = [8; 32];
    assert_eq!(
        reserve_nonce_with(&db.crypto_pool, &physical, || Ok([1; 24])).await?,
        [1; 24]
    );
    assert!(matches!(
        reserve_nonce_with(&db.crypto_pool, &physical, || Ok([1; 24])).await,
        Err(CryptoError::NonceExhausted)
    ));
    let mut attempts = 0;
    assert_eq!(
        reserve_nonce_with(&db.crypto_pool, &physical, || {
            attempts += 1;
            Ok(if attempts < 3 { [1; 24] } else { [2; 24] })
        })
        .await?,
        [2; 24]
    );
    assert_eq!(attempts, 3);
    let mut aborted = db.pool.begin().await?;
    sqlx::query("INSERT INTO workspaces(workspace_id) VALUES($1)")
        .bind(Uuid::from_bytes([9; 16]))
        .execute(&mut *aborted)
        .await?;
    reserve_nonce_with(&db.crypto_pool, &physical, || Ok([3; 24])).await?;
    aborted.rollback().await?;
    assert!(matches!(
        reserve_nonce_with(&db.crypto_pool, &physical, || Ok([3; 24])).await,
        Err(CryptoError::NonceExhausted)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?,
        3
    );
    assert!(
        sqlx::query("DELETE FROM crypto_nonce_reservations")
            .execute(&db.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("TRUNCATE crypto_nonce_reservations")
            .execute(&db.pool)
            .await
            .is_err()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn concurrent_nonce_collision_has_exactly_one_winner(pool: PgPool) -> TestResult {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let physical = [7; 32];
    let (left, right) = tokio::join!(
        reserve_nonce_with(&db.crypto_pool, &physical, || Ok([5; 24])),
        reserve_nonce_with(&db.crypto_pool, &physical, || Ok([5; 24]))
    );
    assert!(matches!(
        (&left, &right),
        (Ok(_), Err(CryptoError::NonceExhausted)) | (Err(CryptoError::NonceExhausted), Ok(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?,
        1
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn durable_document_key_roundtrip_limits_restart_and_creation_races(
    pool: PgPool,
) -> TestResult {
    let mut db = PgDatabase::for_test(pool)?;
    db.keys = Arc::new(provider()?);
    db.migrate().await?;
    let doc = document(2)?;
    let space = workspace(1)?;
    let (left, right) = tokio::join!(
        db.ensure_document_key(space, doc),
        db.ensure_document_key(space, doc)
    );
    left?;
    right?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_crypto_keys")
            .fetch_one(&db.pool)
            .await?,
        1
    );
    assert!(matches!(
        db.ensure_document_key(workspace(3)?, doc).await,
        Err(CryptoError::KeyConflict)
    ));
    let object = Uuid::from_bytes([3; 16]);
    let envelope = db
        .seal_record(doc, 1, "journal", object, 3, b"bounded secret", 14)
        .await?;
    let reopened = PgDatabase {
        pool: db.pool.clone(),
        crypto_pool: db.crypto_pool.clone(),
        keys: Arc::new(provider()?),
    };
    assert_eq!(
        &**reopened
            .open_record(doc, 1, "journal", object, 3, &envelope, 14)
            .await?,
        b"bounded secret"
    );
    assert!(matches!(
        db.seal_record(doc, 1, "journal", object, 3, b"too large", 1)
            .await,
        Err(CryptoError::LimitExceeded { limit: 1 })
    ));
    assert!(matches!(
        db.open_record(doc, 1, "journal", object, 3, &envelope, 1)
            .await,
        Err(CryptoError::LimitExceeded { limit: 1 })
    ));
    assert!(
        db.open_record(doc, 2, "journal", object, 3, &envelope, 14)
            .await
            .is_err()
    );
    assert!(
        db.open_record(doc, 1, "journal", object, 4, &envelope, 14)
            .await
            .is_err()
    );
    // Both the generation PK and the partial one-active-key unique index
    // participate in creation races. Repeat independent first creations so an
    // index-arbiter race cannot hide behind an already existing document key.
    for number in 10..42 {
        let candidate = document(number)?;
        let (left, right) = tokio::join!(
            db.ensure_document_key(space, candidate),
            db.ensure_document_key(space, candidate),
        );
        left?;
        right?;
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_crypto_keys")
            .fetch_one(&db.pool)
            .await?,
        33,
    );
    db.encryption_readiness().await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn retained_generations_and_kek_alias_rewrap_preserve_existing_records(
    pool: PgPool,
) -> TestResult {
    let mut db = PgDatabase::for_test(pool)?;
    db.keys = Arc::new(provider()?);
    db.migrate().await?;
    let doc = document(2)?;
    let space = workspace(1)?;
    db.ensure_document_key(space, doc).await?;
    let object = Uuid::from_bytes([3; 16]);
    let first = db
        .seal_record(doc, 1, "journal", object, 1, b"generation one", 32)
        .await?;
    let old = db.document_key(doc, None).await?;
    let nonce = reserve_nonce_with(
        &db.crypto_pool,
        &db.keys.physical_key_id("alias")?,
        random_bytes::<24>,
    )
    .await?;
    let wrapped = db.keys.wrap_key(
        "alias",
        &nonce,
        &wrapping_aad(space, doc, 1, &old.physical_id),
        &old.material,
    )?;
    sqlx::query("UPDATE document_crypto_keys SET kek_id='alias',wrapped_dek=$1,wrapping_nonce=$2,wrap_revision=2 WHERE document_id=$3").bind(&wrapped).bind(nonce.as_slice()).bind(Uuid::from_bytes(*doc.as_bytes())).execute(&db.pool).await?;
    assert_eq!(
        &**db
            .open_record(doc, 1, "journal", object, 1, &first, 32)
            .await?,
        b"generation one"
    );
    let material = Zeroizing::new([88; 32]);
    let physical = physical_key_id(&material);
    let nonce = reserve_nonce_with(
        &db.crypto_pool,
        &db.keys.physical_key_id("alias")?,
        random_bytes::<24>,
    )
    .await?;
    let wrapped = db.keys.wrap_key(
        "alias",
        &nonce,
        &wrapping_aad(space, doc, 2, &physical),
        &material,
    )?;
    let mut tx = db.pool.begin().await?;
    sqlx::query("UPDATE document_crypto_keys SET active=false WHERE document_id=$1")
        .bind(Uuid::from_bytes(*doc.as_bytes()))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO document_crypto_keys(document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version) VALUES($1,$2,2,$3,'alias',$4,$5,1)").bind(Uuid::from_bytes(*doc.as_bytes())).bind(Uuid::from_bytes(*space.as_bytes())).bind(physical.as_slice()).bind(&wrapped).bind(nonce.as_slice()).execute(&mut *tx).await?;
    tx.commit().await?;
    let second = db
        .seal_record(doc, 2, "journal", object, 1, b"generation two", 32)
        .await?;
    assert_eq!(parse_envelope(&second, 32)?.0, 2);
    assert_eq!(
        &**db
            .open_record(doc, 1, "journal", object, 1, &first, 32)
            .await?,
        b"generation one"
    );
    assert_eq!(
        &**db
            .open_record(doc, 2, "journal", object, 1, &second, 32)
            .await?,
        b"generation two"
    );
    db.encryption_readiness().await?;
    assert!(
        sqlx::query("DELETE FROM document_crypto_keys")
            .execute(&db.pool)
            .await
            .is_err()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn wrong_kek_and_plaintext_new_rows_fail_closed(pool: PgPool) -> TestResult {
    let mut db = PgDatabase::for_test(pool)?;
    db.keys = Arc::new(provider()?);
    db.migrate().await?;
    let doc = document(2)?;
    let space = workspace(1)?;
    db.ensure_document_key(space, doc).await?;
    let object = Uuid::from_bytes([3; 16]);
    let envelope = db
        .seal_record(doc, 1, "journal", object, 1, b"secret", 32)
        .await?;
    let wrong = LocalKeyProvider::from_keyring_json(&KEYRING.replace(
        "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        "2122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40",
    ))?;
    db.keys = Arc::new(wrong);
    assert!(matches!(
        db.open_record(doc, 1, "journal", object, 1, &envelope, 32)
            .await,
        Err(CryptoError::KeyProvider(
            KeyProviderError::AuthenticationFailed
        ))
    ));
    assert!(db.encryption_readiness().await.is_err());
    // The INSERT guard runs before FK validation: no document fixture is
    // needed to prove that a new plaintext row is rejected at this boundary.
    let result = sqlx::query(
        "INSERT INTO document_operations(document_id,committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash) VALUES($1,1,$2,$3,'user',1,$4,$5,$5,$5)",
    )
    .bind(Uuid::from_bytes(*doc.as_bytes()))
    .bind(object)
    .bind(Uuid::from_bytes([4;16]))
    .bind(b"plaintext authored content".as_slice())
    .bind([0_u8;32].as_slice())
    .execute(&db.pool)
    .await;
    assert!(matches!(result, Err(sqlx::Error::Database(ref error)) if error.is_check_violation()));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn crypto_pool_progresses_while_single_document_connection_is_held(
    pool: PgPool,
) -> TestResult {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let document_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_with((*db.pool.connect_options()).clone())
        .await?;
    let narrow = PgDatabase {
        pool: document_pool,
        crypto_pool: db.crypto_pool.clone(),
        keys: db.keys.clone(),
    };
    let doc = document(2)?;
    narrow.ensure_document_key(workspace(1)?, doc).await?;
    let held = narrow.begin_guarded().await?;
    let encrypted = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        narrow.seal_record_guarded(
            doc,
            1,
            "journal",
            Uuid::from_bytes([3; 16]),
            1,
            b"independent durable reservation",
            64,
        ),
    )
    .await??;
    assert!(parse_envelope(&encrypted, 64).is_ok());
    held.rollback().await?;
    narrow.pool.close().await;
    Ok(())
}
