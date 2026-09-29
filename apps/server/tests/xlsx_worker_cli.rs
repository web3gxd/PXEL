use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/xlsx/minimal_scalars.xlsx")
}

#[test]
fn preview_runs_in_child_and_returns_counts_only() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .arg("xlsx-preview")
        .arg(fixture())
        .output()?;
    assert!(output.status.success());
    let stdout = std::str::from_utf8(&output.stdout)?;
    assert_eq!(stdout.trim(), "XLSX preview: 2 sheets, 5 cells");
    assert!(!stdout.contains("hello"));
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn malformed_file_is_rejected_without_echoing_content() -> Result<(), Box<dyn std::error::Error>> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), b"private-cell-value-not-a-zip")?;
    let output = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .arg("xlsx-preview")
        .arg(file.path())
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = std::str::from_utf8(&output.stderr)?;
    assert!(stderr.contains("XLSX worker rejected the package"));
    assert!(!stderr.contains("private-cell-value"));
    Ok(())
}

#[test]
fn child_rejects_unknown_request_version_before_parsing() -> Result<(), Box<dyn std::error::Error>>
{
    let mut child = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .arg("xlsx-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"PXIW\0\x02\0\0")?;
    }
    let output = child.wait_with_output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!std::str::from_utf8(&output.stderr)?.contains("private-cell-value"));
    Ok(())
}
