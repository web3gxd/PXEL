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
        stdin.write_all(b"PXIW\0\x04\0\0\0\x03abc")?;
    }
    let output = child.wait_with_output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!std::str::from_utf8(&output.stderr)?.contains("private-cell-value"));
    Ok(())
}

fn byte_worker(frame: &[u8]) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pxel-server"))
        .arg("xlsx-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(frame)?;
    }
    Ok(child.wait_with_output()?)
}

#[test]
fn child_parses_package_bytes_without_a_file_path() -> Result<(), Box<dyn std::error::Error>> {
    let source = std::fs::read(fixture())?;
    let mut frame = b"PXIW\0\x05".to_vec();
    frame.extend_from_slice(&u32::try_from(source.len())?.to_be_bytes());
    frame.extend_from_slice(&source);
    let output = byte_worker(&frame)?;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() > 11);
    assert_eq!(&output.stdout[..7], b"PXOW\0\x05\0");
    let length = u32::from_be_bytes(output.stdout[7..11].try_into()?) as usize;
    assert_eq!(output.stdout.len(), 11 + length);
    let book: spreadsheet_xlsx::NormalizedWorkbook = serde_json::from_slice(&output.stdout[11..])?;
    book.validate()?;
    assert_eq!(book.sheets.len(), 2);
    assert_eq!(book.sheets.iter().map(|s| s.cells.len()).sum::<usize>(), 5);
    Ok(())
}

#[test]
fn child_rejects_invalid_byte_frames_without_echoing_input()
-> Result<(), Box<dyn std::error::Error>> {
    for frame in [
        b"PXIW\0\x05\0\0\0\0".as_slice(),
        b"PXIW\0\x05\x02\0\0\x01".as_slice(),
        b"PXIW\0\x05\0\0\0\x03ab".as_slice(),
        b"PXIW\0\x05\0\0\0\x03abcd".as_slice(),
    ] {
        let output = byte_worker(frame)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("abc"));
    }
    Ok(())
}
