//! A bounded local process boundary for XLSX parsing. OS sandbox adapters are
//! a later gate; only a bounded, parent-validated IR crosses this boundary.

use spreadsheet_xlsx::NormalizedWorkbook;
use std::{
    io::{Read, Write},
    path::Path,
    process::Stdio,
    time::Duration,
};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const REQUEST_MAGIC: &[u8; 4] = b"PXIW";
const RESPONSE_MAGIC: &[u8; 4] = b"PXOW";
const PROTOCOL_VERSION: u16 = 3;
const MAX_PATH_BYTES: usize = 4096;
const RESPONSE_HEADER_BYTES: usize = 11;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const WORKER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error, Eq, PartialEq)]
pub enum WorkerError {
    #[error("XLSX worker input is invalid")]
    InvalidInput,
    #[error("XLSX worker rejected the package")]
    Rejected,
    #[error("XLSX worker protocol failed")]
    Protocol,
    #[error("XLSX worker exceeded its deadline")]
    Timeout,
    #[error("XLSX worker could not start or complete")]
    Process,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewCounts {
    pub sheets: u32,
    pub cells: u32,
}

/// Parent-side entry point. Only a canonical absolute path crosses the boundary.
pub async fn preview(path: &Path) -> Result<PreviewCounts, WorkerError> {
    let workbook = load(path).await?;
    let sheets = u32::try_from(workbook.sheets.len()).map_err(|_| WorkerError::Protocol)?;
    let cells = workbook
        .sheets
        .iter()
        .try_fold(0_u32, |sum, sheet| {
            u32::try_from(sheet.cells.len())
                .ok()
                .and_then(|len| sum.checked_add(len))
        })
        .ok_or(WorkerError::Protocol)?;
    Ok(PreviewCounts { sheets, cells })
}

/// Decode and validate worker output again before it can enter server workflows.
pub async fn load(path: &Path) -> Result<NormalizedWorkbook, WorkerError> {
    let absolute = tokio::fs::canonicalize(path)
        .await
        .map_err(|_| WorkerError::InvalidInput)?;
    let path_bytes = absolute
        .to_str()
        .ok_or(WorkerError::InvalidInput)?
        .as_bytes();
    if path_bytes.is_empty() || path_bytes.len() > MAX_PATH_BYTES {
        return Err(WorkerError::InvalidInput);
    }
    let path_len = u16::try_from(path_bytes.len()).map_err(|_| WorkerError::InvalidInput)?;
    let executable = std::env::current_exe().map_err(|_| WorkerError::Process)?;
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("xlsx-worker")
        .env_clear()
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    let mut child = command.spawn().map_err(|_| WorkerError::Process)?;
    let mut stdin = child.stdin.take().ok_or(WorkerError::Process)?;
    let mut stdout = child.stdout.take().ok_or(WorkerError::Process)?;
    let transfer = tokio::time::timeout(WORKER_TIMEOUT, async {
        stdin
            .write_all(REQUEST_MAGIC)
            .await
            .map_err(|_| WorkerError::Protocol)?;
        stdin
            .write_all(&PROTOCOL_VERSION.to_be_bytes())
            .await
            .map_err(|_| WorkerError::Protocol)?;
        stdin
            .write_all(&path_len.to_be_bytes())
            .await
            .map_err(|_| WorkerError::Protocol)?;
        stdin
            .write_all(path_bytes)
            .await
            .map_err(|_| WorkerError::Protocol)?;
        stdin.shutdown().await.map_err(|_| WorkerError::Protocol)?;
        drop(stdin);
        let mut frame = [0_u8; RESPONSE_HEADER_BYTES];
        stdout
            .read_exact(&mut frame)
            .await
            .map_err(|_| WorkerError::Protocol)?;
        let length = decode_header(&frame)?;
        let mut payload = vec![0_u8; length];
        stdout
            .read_exact(&mut payload)
            .await
            .map_err(|_| WorkerError::Protocol)?;
        let mut extra = [0_u8; 1];
        if stdout
            .read(&mut extra)
            .await
            .map_err(|_| WorkerError::Protocol)?
            != 0
        {
            return Err(WorkerError::Protocol);
        }
        let status = child.wait().await.map_err(|_| WorkerError::Process)?;
        if !status.success() {
            return Err(WorkerError::Process);
        }
        decode_payload(&payload)
    })
    .await;
    match transfer {
        Ok(Ok(counts)) => Ok(counts),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(WorkerError::Timeout)
        }
    }
}

/// Child-side entry point. Output is one bounded versioned IR frame.
pub fn run() -> Result<(), WorkerError> {
    let mut stdin = std::io::stdin().lock();
    let mut header = [0_u8; 8];
    stdin
        .read_exact(&mut header)
        .map_err(|_| WorkerError::Protocol)?;
    if &header[..4] != REQUEST_MAGIC
        || u16::from_be_bytes([header[4], header[5]]) != PROTOCOL_VERSION
    {
        return Err(WorkerError::Protocol);
    }
    let path_len = usize::from(u16::from_be_bytes([header[6], header[7]]));
    if path_len == 0 || path_len > MAX_PATH_BYTES {
        return Err(WorkerError::InvalidInput);
    }
    let mut path = vec![0_u8; path_len];
    stdin
        .read_exact(&mut path)
        .map_err(|_| WorkerError::Protocol)?;
    let mut extra = [0_u8; 1];
    if stdin.read(&mut extra).map_err(|_| WorkerError::Protocol)? != 0 {
        return Err(WorkerError::Protocol);
    }
    let path = std::str::from_utf8(&path).map_err(|_| WorkerError::InvalidInput)?;
    let result = spreadsheet_xlsx::normalize_file(Path::new(path));
    let (status, payload) = match result {
        Ok(workbook) => {
            workbook.validate().map_err(|_| WorkerError::Protocol)?;
            let payload = serde_json::to_vec(&workbook).map_err(|_| WorkerError::Protocol)?;
            if payload.len() > MAX_RESPONSE_BYTES {
                return Err(WorkerError::Rejected);
            }
            (0_u8, payload)
        }
        Err(_) => (1_u8, Vec::new()),
    };
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&encode_header(status, payload.len())?)
        .map_err(|_| WorkerError::Protocol)?;
    stdout
        .write_all(&payload)
        .map_err(|_| WorkerError::Protocol)?;
    stdout.flush().map_err(|_| WorkerError::Protocol)?;
    Ok(())
}

fn encode_header(status: u8, length: usize) -> Result<[u8; RESPONSE_HEADER_BYTES], WorkerError> {
    if length > MAX_RESPONSE_BYTES || (status == 0 && length == 0) || (status != 0 && length != 0) {
        return Err(WorkerError::Protocol);
    }
    let mut frame = [0_u8; RESPONSE_HEADER_BYTES];
    frame[..4].copy_from_slice(RESPONSE_MAGIC);
    frame[4..6].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    frame[6] = status;
    let length = u32::try_from(length).map_err(|_| WorkerError::Protocol)?;
    frame[7..11].copy_from_slice(&length.to_be_bytes());
    Ok(frame)
}

fn decode_header(frame: &[u8; RESPONSE_HEADER_BYTES]) -> Result<usize, WorkerError> {
    if &frame[..4] != RESPONSE_MAGIC || u16::from_be_bytes([frame[4], frame[5]]) != PROTOCOL_VERSION
    {
        return Err(WorkerError::Protocol);
    }
    if frame[6] == 1 {
        return Err(WorkerError::Rejected);
    }
    if frame[6] != 0 {
        return Err(WorkerError::Protocol);
    }
    let length = u32::from_be_bytes([frame[7], frame[8], frame[9], frame[10]]) as usize;
    if length == 0 || length > MAX_RESPONSE_BYTES {
        return Err(WorkerError::Protocol);
    }
    Ok(length)
}

fn decode_payload(payload: &[u8]) -> Result<NormalizedWorkbook, WorkerError> {
    if payload.is_empty() || payload.len() > MAX_RESPONSE_BYTES {
        return Err(WorkerError::Protocol);
    }
    let workbook: NormalizedWorkbook =
        serde_json::from_slice(payload).map_err(|_| WorkerError::Protocol)?;
    workbook.validate().map_err(|_| WorkerError::Protocol)?;
    Ok(workbook)
}

#[cfg(test)]
mod tests {
    use super::{WorkerError, decode_header, decode_payload, encode_header};

    #[test]
    fn response_codec_rejects_unknown_version_status_and_size() {
        let frame = encode_header(0, 5).unwrap_or([0; 11]);
        assert_eq!(decode_header(&frame), Ok(5));
        let mut unknown = frame;
        unknown[5] = 4;
        assert_eq!(decode_header(&unknown), Err(WorkerError::Protocol));
        let rejected = encode_header(1, 0).unwrap_or([0; 11]);
        assert_eq!(decode_header(&rejected), Err(WorkerError::Rejected));
        assert_eq!(encode_header(0, 0), Err(WorkerError::Protocol));
        assert_eq!(
            encode_header(0, 64 * 1024 * 1024 + 1),
            Err(WorkerError::Protocol)
        );
    }

    #[test]
    fn parent_rejects_invalid_worker_ir() {
        let valid = br#"{"sheets":[{"name":"Data","cells":[{"row":1,"column":1,"value":{"Text":"hello"}}]}],"issues":[]}"#;
        assert!(decode_payload(valid).is_ok());
        let duplicate = br#"{"sheets":[{"name":"Data","cells":[{"row":1,"column":1,"value":{"Text":"a"}},{"row":1,"column":1,"value":{"Text":"b"}}]}],"issues":[]}"#;
        assert_eq!(decode_payload(duplicate), Err(WorkerError::Protocol));
        let invalid_number = br#"{"sheets":[{"name":"Data","cells":[{"row":1,"column":1,"value":{"Number":-0.0}}]}],"issues":[]}"#;
        assert_eq!(decode_payload(invalid_number), Err(WorkerError::Protocol));
        let unknown = br#"{"sheets":[{"name":"Data","cells":[],"hidden":true}],"issues":[]}"#;
        assert_eq!(decode_payload(unknown), Err(WorkerError::Protocol));
        let duplicate_names =
            br#"{"sheets":[{"name":"Data","cells":[]},{"name":"data","cells":[]}],"issues":[]}"#;
        assert_eq!(decode_payload(duplicate_names), Err(WorkerError::Protocol));
        let invented_code = br#"{"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"Unknown","sheet_index":0}]}"#;
        assert_eq!(decode_payload(invented_code), Err(WorkerError::Protocol));
        let invalid_sheet = br#"{"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"CellFormatting","sheet_index":1}]}"#;
        assert_eq!(decode_payload(invalid_sheet), Err(WorkerError::Protocol));
        let duplicated_issue = br#"{"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"CellFormatting","sheet_index":0},{"code":"CellFormatting","sheet_index":0}]}"#;
        assert_eq!(decode_payload(duplicated_issue), Err(WorkerError::Protocol));
        let mislabeled_issue = br#"{"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"WorkbookMetadata","sheet_index":0}]}"#;
        assert_eq!(decode_payload(mislabeled_issue), Err(WorkerError::Protocol));
    }
}
