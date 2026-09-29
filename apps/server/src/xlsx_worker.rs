//! A bounded local process boundary for XLSX parsing. OS sandbox adapters are
//! a later gate; this protocol never carries document content back to parent.

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
const PROTOCOL_VERSION: u16 = 1;
const MAX_PATH_BYTES: usize = 4096;
const RESPONSE_BYTES: usize = 15;
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
        let mut frame = [0_u8; RESPONSE_BYTES];
        stdout
            .read_exact(&mut frame)
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
        decode_response(&frame)
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

/// Child-side entry point. The only result is a fixed-size versioned frame.
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
    let (status, counts) = match result {
        Ok(workbook) => {
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
            (0_u8, PreviewCounts { sheets, cells })
        }
        Err(_) => (
            1_u8,
            PreviewCounts {
                sheets: 0,
                cells: 0,
            },
        ),
    };
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&encode_response(status, counts))
        .map_err(|_| WorkerError::Protocol)?;
    stdout.flush().map_err(|_| WorkerError::Protocol)?;
    Ok(())
}

fn encode_response(status: u8, counts: PreviewCounts) -> [u8; RESPONSE_BYTES] {
    let mut frame = [0_u8; RESPONSE_BYTES];
    frame[..4].copy_from_slice(RESPONSE_MAGIC);
    frame[4..6].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    frame[6] = status;
    frame[7..11].copy_from_slice(&counts.sheets.to_be_bytes());
    frame[11..15].copy_from_slice(&counts.cells.to_be_bytes());
    frame
}

fn decode_response(frame: &[u8; RESPONSE_BYTES]) -> Result<PreviewCounts, WorkerError> {
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
    let sheets = u32::from_be_bytes([frame[7], frame[8], frame[9], frame[10]]);
    let cells = u32::from_be_bytes([frame[11], frame[12], frame[13], frame[14]]);
    if sheets == 0 || sheets > 64 || cells > 100_000 {
        return Err(WorkerError::Protocol);
    }
    Ok(PreviewCounts { sheets, cells })
}

#[cfg(test)]
mod tests {
    use super::{PreviewCounts, WorkerError, decode_response, encode_response};

    #[test]
    fn response_codec_rejects_unknown_version_status_and_counts() {
        let counts = PreviewCounts {
            sheets: 2,
            cells: 5,
        };
        let frame = encode_response(0, counts);
        assert_eq!(decode_response(&frame), Ok(counts));
        let mut unknown = frame;
        unknown[5] = 2;
        assert_eq!(decode_response(&unknown), Err(WorkerError::Protocol));
        let rejected = encode_response(
            1,
            PreviewCounts {
                sheets: 0,
                cells: 0,
            },
        );
        assert_eq!(decode_response(&rejected), Err(WorkerError::Rejected));
        let invalid = encode_response(
            0,
            PreviewCounts {
                sheets: 65,
                cells: 0,
            },
        );
        assert_eq!(decode_response(&invalid), Err(WorkerError::Protocol));
    }
}
