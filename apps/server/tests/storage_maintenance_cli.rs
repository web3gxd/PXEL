#![allow(clippy::expect_used)]

use sha2::{Digest, Sha256};
use spreadsheet_storage::{LocalKeyProvider, PgDatabase};
use std::{path::Path, process::Command, sync::Arc};

fn make_private_directory(path: &Path) {
    #[cfg(windows)]
    {
        let script = r#"$ErrorActionPreference='Stop'; try {
          $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;
          $allowed=@($sid.Value,'S-1-5-18','S-1-5-32-544');
          $acl=[System.Security.AccessControl.DirectorySecurity]::new();
          $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($sid);
          foreach($value in $allowed) {
            $identity=[System.Security.Principal.SecurityIdentifier]::new($value);
            $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($identity,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Allow);
            $acl.AddAccessRule($rule)
          };
          Set-Acl -LiteralPath $env:PXEL_TEST_PRIVATE_DIR -AclObject $acl; exit 0
        } catch { exit 1 }"#;
        use std::os::windows::process::CommandExt;
        let system_root = std::env::var_os("SystemRoot").expect("Windows system root");
        let executable =
            Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let result = Command::new(executable)
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env_clear()
            .env("SystemRoot", system_root)
            .env("PXEL_TEST_PRIVATE_DIR", path)
            .creation_flags(0x0800_0000)
            .status()
            .expect("PowerShell starts to secure fixture directory");
        assert!(result.success(), "fixture ACL setup failed");
    }
    #[cfg(not(windows))]
    let _ = path;
}

fn keyring(material: &str) -> String {
    format!(
        r#"{{"schema_version":1,"active_kek_id":"cli-test","keys":[{{"id":"cli-test","key_hex":"{}"}}]}}"#,
        material.repeat(32)
    )
}

fn verify(archive: &Path, keyring: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .args(["backup", "verify"])
        .arg(archive)
        .env_remove("DATABASE_URL")
        .env(
            "PXEL_DATABASE_URL_FILE",
            "missing-backup-test-configuration",
        )
        .env_remove("PXEL_KEYRING_FILE")
        .env("PXEL_KEYRING_JSON", keyring)
        .output()
        .expect("test binary starts")
}

fn assert_cli_logs_omit(output: &std::process::Output, marker: &[u8]) {
    for stream in [&output.stdout, &output.stderr] {
        assert!(
            !stream.windows(marker.len()).any(|window| window == marker),
            "maintenance CLI output contained synthetic document bytes"
        );
    }
}

#[test]
#[ignore = "requires PXEL_BACKUP_CLI_TEST_DATABASE_URL and PXEL_KEYRING_FILE"]
fn wal_archive_cli_is_authenticated_idempotent_and_never_publishes_bad_plaintext() {
    let url = std::env::var_os("PXEL_BACKUP_CLI_TEST_DATABASE_URL")
        .expect("dedicated test database is configured");
    let keyring = std::env::var_os("PXEL_KEYRING_FILE").expect("private keyring is configured");
    let keyring_parent = Path::new(&keyring).parent().expect("keyring parent");
    let temporary = tempfile::Builder::new()
        .prefix("pxel-wal-cli-")
        .tempdir_in(keyring_parent)
        .expect("private WAL fixture");
    let source_directory = temporary.path().join("source/pg_wal");
    let archive_directory = temporary.path().join("archive");
    let restored_directory = temporary.path().join("restored/pg_wal");
    let rejected_directory = temporary.path().join("rejected/pg_wal");
    for directory in [
        &source_directory,
        &archive_directory,
        &restored_directory,
        &rejected_directory,
    ] {
        std::fs::create_dir_all(directory).expect("fixture directory");
    }
    make_private_directory(&archive_directory);
    make_private_directory(&restored_directory);
    make_private_directory(&rejected_directory);
    let name = "000000010000000000000001";
    let wal = b"synthetic encrypted WAL segment do-not-echo-fixture";
    let source = source_directory.join(name);
    std::fs::write(&source, wal).expect("source WAL segment");
    let archive = archive_directory.join(format!("{name}.pxel"));
    let archived = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .args(["backup", "wal-archive"])
        .arg(&archive_directory)
        .arg(&source)
        .arg(name)
        .env("DATABASE_URL", url)
        .env_remove("PXEL_DATABASE_URL_FILE")
        .env("PXEL_KEYRING_FILE", &keyring)
        .env_remove("PXEL_KEYRING_JSON")
        .output()
        .expect("WAL archiver process starts");
    assert_cli_logs_omit(&archived, wal);
    assert!(archived.status.success(), "WAL archiver failed");
    let cipher = std::fs::read(&archive).expect("published ciphertext");
    assert!(
        !cipher
            .windows(b"do-not-echo-fixture".len())
            .any(|window| window == b"do-not-echo-fixture")
    );

    let retry = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .args(["backup", "wal-archive"])
        .arg(&archive_directory)
        .arg(&source)
        .arg(name)
        .env(
            "DATABASE_URL",
            std::env::var_os("PXEL_BACKUP_CLI_TEST_DATABASE_URL").expect("test URL"),
        )
        .env_remove("PXEL_DATABASE_URL_FILE")
        .env("PXEL_KEYRING_FILE", &keyring)
        .env_remove("PXEL_KEYRING_JSON")
        .output()
        .expect("idempotent archiver retry starts");
    assert_cli_logs_omit(&retry, wal);
    assert!(retry.status.success(), "WAL archiver retry failed");
    assert_eq!(std::fs::read(&archive).expect("same ciphertext"), cipher);

    let restored = restored_directory.join(name);
    let restore = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .args(["backup", "wal-restore"])
        .arg(&archive_directory)
        .arg(name)
        .arg(&restored)
        .env_remove("DATABASE_URL")
        .env_remove("PXEL_DATABASE_URL_FILE")
        .env("PXEL_KEYRING_FILE", &keyring)
        .env_remove("PXEL_KEYRING_JSON")
        .output()
        .expect("WAL restore helper starts");
    assert_cli_logs_omit(&restore, wal);
    assert!(restore.status.success(), "WAL restore failed");
    assert_eq!(std::fs::read(&restored).expect("restored WAL"), wal);

    let preexisting = &restored;
    std::fs::write(preexisting, b"stale WAL copied from the physical base")
        .expect("preexisting WAL destination");
    let replace = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .args(["backup", "wal-restore"])
        .arg(&archive_directory)
        .arg(name)
        .arg(preexisting)
        .env_remove("DATABASE_URL")
        .env_remove("PXEL_DATABASE_URL_FILE")
        .env("PXEL_KEYRING_FILE", &keyring)
        .env_remove("PXEL_KEYRING_JSON")
        .output()
        .expect("WAL restore can replace the physical-base segment");
    assert_cli_logs_omit(&replace, wal);
    assert!(replace.status.success(), "WAL replacement failed");
    assert_eq!(std::fs::read(preexisting).expect("replaced WAL"), wal);

    let mut damaged = cipher;
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    std::fs::write(&archive, damaged).expect("tampered ciphertext");
    let rejected = rejected_directory.join(name);
    let failed = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .args(["backup", "wal-restore"])
        .arg(&archive_directory)
        .arg(name)
        .arg(&rejected)
        .env_remove("DATABASE_URL")
        .env_remove("PXEL_DATABASE_URL_FILE")
        .env("PXEL_KEYRING_FILE", &keyring)
        .env_remove("PXEL_KEYRING_JSON")
        .output()
        .expect("tampered restore helper starts");
    assert_cli_logs_omit(&failed, wal);
    assert!(!failed.status.success());
    assert!(!rejected.exists());
    assert!(
        std::fs::read_dir(&rejected_directory)
            .expect("rejected restore directory")
            .next()
            .is_none()
    );
}

#[test]
fn invalid_backup_arguments_fail_before_configuration_is_loaded() {
    for arguments in [
        vec!["backup"],
        vec!["backup", "restore", "unused"],
        vec!["backup", "create", "unused"],
        vec!["backup", "create", "unused", "--unknown"],
        vec!["backup", "verify"],
        vec!["backup", "verify", "unused", "extra"],
        vec!["backup", "verify", "--maintenance-confirmed"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
            .args(arguments)
            .env_remove("DATABASE_URL")
            .env(
                "PXEL_DATABASE_URL_FILE",
                "missing-backup-test-configuration",
            )
            .env_remove("PXEL_KEYRING_JSON")
            .env_remove("PXEL_KEYRING_FILE")
            .output()
            .expect("test binary starts");
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("usage: pxel-server backup"));
        assert!(!error.contains("missing-backup-test-configuration"));
    }
}

#[test]
fn invalid_and_oversized_database_urls_fail_without_echoing_credentials() {
    for url in [
        " ".to_owned(),
        "invalid-do-not-echo-credential".to_owned(),
        format!(
            "postgres://owner:{}@localhost/db",
            "do-not-echo-credential".repeat(900)
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
            .args(["backup", "create", "unused", "--maintenance-confirmed"])
            .env("DATABASE_URL", url)
            .env_remove("PXEL_PG_BIN")
            .env_remove("PXEL_KEYRING_FILE")
            .env_remove("PXEL_KEYRING_JSON")
            .output()
            .expect("test binary starts");
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("backup configuration is invalid or unavailable"));
        assert!(!error.contains("do-not-echo-credential"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn unknown_format_version_and_truncated_archives_fail_without_database_or_content_output() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let archive = directory.path().join("fixture.pxel-backup");
    let mut unknown_version = b"PXELBKP\0".to_vec();
    unknown_version.extend_from_slice(&2_u16.to_be_bytes());
    unknown_version.extend_from_slice(&1_u16.to_be_bytes());
    unknown_version.extend_from_slice(&65536_u32.to_be_bytes());
    unknown_version.extend_from_slice(&8_u16.to_be_bytes());
    for bytes in [
        b"unrecognized-format-do-not-echo-fixture".to_vec(),
        unknown_version,
        b"PXELBKP\0".to_vec(),
    ] {
        std::fs::write(&archive, bytes).expect("fixture archive");
        let output = verify(&archive, &keyring("11"));
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!error.contains("do-not-echo-fixture"));
        assert!(!error.contains("missing-backup-test-configuration"));
        assert!(!error.contains("key_hex"));
        assert!(output.stdout.is_empty());
    }
}

#[tokio::test]
#[ignore = "requires PXEL_BACKUP_CLI_TEST_DATABASE_URL for a dedicated migrated cluster"]
async fn valid_bundle_verifies_and_wrong_key_truncation_trailing_data_and_manifest_mismatch_fail() {
    let url = std::env::var("PXEL_BACKUP_CLI_TEST_DATABASE_URL")
        .expect("explicit dedicated test database is configured");
    let correct = keyring("11");
    let keys =
        Arc::new(LocalKeyProvider::from_keyring_json(&correct).expect("synthetic test keyring"));
    let database = PgDatabase::connect_with_keys(&url, 2, keys)
        .await
        .expect("dedicated database connects");
    let temporary = tempfile::tempdir().expect("fixture directory");
    let archive = temporary.path().join("encrypted.fixture");
    let pg = b"synthetic PostgreSQL tar bytes do-not-echo-fixture";
    let mut payload = b"PXELBND1".to_vec();
    payload.extend_from_slice(&(pg.len() as u32).to_be_bytes());
    payload.extend_from_slice(pg);
    payload.extend_from_slice(&0_u32.to_be_bytes());
    payload.extend_from_slice(&(pg.len() as u64).to_be_bytes());
    payload.extend_from_slice(&Sha256::digest(pg));
    payload.extend_from_slice(&0_u32.to_be_bytes());
    payload.extend_from_slice(b"PXELEND1");
    let manifest = br#"{"format_version":1,"postgres_version":"18.6","blobs":[]}"#;
    let mut ciphertext = Vec::new();
    Box::pin(database.encrypt_backup(manifest, &mut payload.as_slice(), &mut ciphertext))
        .await
        .expect("synthetic bundle encryption");
    std::fs::write(&archive, &ciphertext).expect("fixture archive");
    assert!(verify(&archive, &correct).status.success());
    assert!(!verify(&archive, &keyring("22")).status.success());
    std::fs::write(&archive, &ciphertext[..ciphertext.len() - 1]).expect("truncated fixture");
    assert!(!verify(&archive, &correct).status.success());
    let mut appended = ciphertext;
    appended.extend_from_slice(b"do-not-echo-fixture");
    std::fs::write(&archive, appended).expect("appended fixture");
    let output = verify(&archive, &correct);
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("do-not-echo-fixture"));
    let bad_manifest = br#"{"format_version":2,"postgres_version":"18.6","blobs":[]}"#;
    let mut ciphertext = Vec::new();
    Box::pin(database.encrypt_backup(bad_manifest, &mut payload.as_slice(), &mut ciphertext))
        .await
        .expect("authenticated unsupported manifest fixture");
    std::fs::write(&archive, ciphertext).expect("fixture archive");
    assert!(!verify(&archive, &correct).status.success());
    database.close().await;
}
