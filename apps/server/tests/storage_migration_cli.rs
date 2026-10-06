#![allow(clippy::expect_used)]

use std::process::{Command, Output};

fn command(directory: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pxel-server"));
    command.current_dir(directory).env_clear();
    for variable in ["SystemRoot", "PATH", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
    command
}

fn rejected(output: Output, expected: &str, directory: &std::path::Path) {
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains(expected), "unexpected safe error: {error}");
    assert!(!error.contains("do-not-echo"));
    assert!(!error.contains("postgres://"));
    assert!(!error.contains("key_hex"));
    assert!(output.stdout.is_empty());
    assert_eq!(
        std::fs::read_dir(directory)
            .expect("fixture directory")
            .count(),
        0
    );
}

#[cfg(feature = "legacy-migration")]
fn configured(command: &mut Command) {
    command
        .env(
            "PXEL_LEGACY_SOURCE_DATABASE_URL",
            "postgres://owner:do-not-echo-password@localhost:35481/legacy",
        )
        .env(
            "DATABASE_URL",
            "postgres://owner:do-not-echo-target-password@localhost:35482/encrypted",
        )
        .env(
            "PXEL_RESTORE_DATABASE_URL",
            "postgres://owner:do-not-echo-password@localhost:35483/legacy",
        )
        .env("PXEL_LEGACY_SOURCE_BLOB_ROOT", "do-not-echo-source-blobs")
        .env("PXEL_BLOB_ROOT", "do-not-echo-target-blobs")
        .env("PXEL_MIGRATION_ROLLBACK_ARCHIVE", "do-not-echo-archive")
        .env(
            "PXEL_MIGRATION_VERIFY_ROOT",
            "do-not-echo-verification-root",
        )
        .env("PXEL_KEYRING_JSON", "do-not-echo-invalid-key-material");
}

#[cfg(feature = "legacy-migration")]
#[test]
fn exact_arguments_are_required_before_configuration_is_loaded() {
    let directory = tempfile::tempdir().expect("fixture directory");
    for arguments in [
        vec!["storage"],
        vec!["storage", "migrate-legacy"],
        vec!["storage", "migrate-legacy", "--unknown"],
        vec![
            "storage",
            "migrate-legacy",
            "--maintenance-confirmed",
            "extra",
        ],
        vec![
            "storage",
            "migrate-legacy",
            "--maintenance-confirmed",
            "--maintenance-confirmed",
        ],
        vec!["storage", "unknown", "--maintenance-confirmed"],
    ] {
        let mut child = command(directory.path());
        configured(&mut child);
        rejected(
            child.args(arguments).output().expect("test binary starts"),
            "usage: pxel-server storage migrate-legacy --maintenance-confirmed",
            directory.path(),
        );
    }
}

#[cfg(feature = "legacy-migration")]
#[test]
fn missing_and_invalid_configuration_fail_without_database_or_file_mutations() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let arguments = ["storage", "migrate-legacy", "--maintenance-confirmed"];
    rejected(
        command(directory.path())
            .args(arguments)
            .output()
            .expect("test binary starts"),
        "legacy migration configuration is invalid or unavailable",
        directory.path(),
    );
    for url in [
        "do-not-echo-invalid-url".to_owned(),
        " ".to_owned(),
        format!(
            "postgres://owner:{}@localhost:35481/legacy",
            "do-not-echo".repeat(2000)
        ),
    ] {
        let mut child = command(directory.path());
        configured(&mut child);
        rejected(
            child
                .args(arguments)
                .env("PXEL_LEGACY_SOURCE_DATABASE_URL", url)
                .output()
                .expect("test binary starts"),
            "legacy migration configuration is invalid or unavailable",
            directory.path(),
        );
    }
    for missing in [
        "DATABASE_URL",
        "PXEL_RESTORE_DATABASE_URL",
        "PXEL_LEGACY_SOURCE_BLOB_ROOT",
        "PXEL_BLOB_ROOT",
        "PXEL_MIGRATION_ROLLBACK_ARCHIVE",
        "PXEL_MIGRATION_VERIFY_ROOT",
    ] {
        let mut child = command(directory.path());
        configured(&mut child);
        rejected(
            child
                .args(arguments)
                .env_remove(missing)
                .output()
                .expect("test binary starts"),
            "legacy migration configuration is invalid or unavailable",
            directory.path(),
        );
    }
}

#[cfg(feature = "legacy-migration")]
#[test]
fn identical_clusters_are_rejected_even_with_different_loopback_names() {
    let directory = tempfile::tempdir().expect("fixture directory");
    for (index, target) in [
        "postgres://owner:do-not-echo-password@localhost:35481/legacy",
        "postgres://other:do-not-echo-other-password@127.0.0.1:35481/other",
        "postgres://other:do-not-echo-other-password@[::1]:35481/other",
    ]
    .into_iter()
    .enumerate()
    {
        let mut child = command(directory.path());
        configured(&mut child);
        let output = child
            .args(["storage", "migrate-legacy", "--maintenance-confirmed"])
            .env("DATABASE_URL", target)
            .output()
            .expect("test binary starts");
        let error = String::from_utf8_lossy(&output.stderr);
        let expected = if index == 0 {
            "legacy migration requires distinct local PostgreSQL clusters"
        } else {
            assert!(
                error.contains("legacy migration requires distinct local PostgreSQL clusters")
                    || error.contains("legacy migration configuration is invalid or unavailable"),
                "loopback alias case {index} was not rejected safely: {error}"
            );
            if error.contains("legacy migration requires distinct local PostgreSQL clusters") {
                "legacy migration requires distinct local PostgreSQL clusters"
            } else {
                "legacy migration configuration is invalid or unavailable"
            }
        };
        rejected(output, expected, directory.path());
    }
}

#[cfg(feature = "legacy-migration")]
#[test]
fn recovery_requires_a_separate_port_and_the_original_source_credentials() {
    let directory = tempfile::tempdir().expect("fixture directory");
    for recovery in [
        "postgres://owner:do-not-echo-password@localhost:35481/legacy",
        "postgres://owner:do-not-echo-password@localhost:35482/legacy",
        "postgres://other:do-not-echo-password@localhost:35483/legacy",
        "postgres://owner:do-not-echo-wrong-password@localhost:35483/legacy",
        "postgres://owner:do-not-echo-password@localhost:35483/other",
        "postgres://owner:do-not-echo-password@localhost:35483/legacy?sslmode=require",
    ] {
        let mut child = command(directory.path());
        configured(&mut child);
        rejected(
            child
                .args(["storage", "migrate-legacy", "--maintenance-confirmed"])
                .env("PXEL_RESTORE_DATABASE_URL", recovery)
                .output()
                .expect("test binary starts"),
            "legacy migration configuration is invalid or unavailable",
            directory.path(),
        );
    }
}

#[cfg(feature = "legacy-migration")]
#[test]
fn unsupported_blob_backend_and_invalid_keyring_fail_before_postgresql_is_contacted() {
    let directory = tempfile::tempdir().expect("fixture directory");
    for s3 in [false, true] {
        let mut child = command(directory.path());
        configured(&mut child);
        if s3 {
            child.env("PXEL_S3_ENDPOINT", "do-not-echo-endpoint");
        }
        rejected(
            child
                .args(["storage", "migrate-legacy", "--maintenance-confirmed"])
                .output()
                .expect("test binary starts"),
            "legacy migration configuration is invalid or unavailable",
            directory.path(),
        );
    }
}

#[cfg(not(feature = "legacy-migration"))]
#[test]
fn migration_is_disabled_in_the_ordinary_server_build() {
    let directory = tempfile::tempdir().expect("fixture directory");
    rejected(
        command(directory.path())
            .args(["storage", "migrate-legacy", "--maintenance-confirmed"])
            .output()
            .expect("test binary starts"),
        "legacy-migration",
        directory.path(),
    );
}
