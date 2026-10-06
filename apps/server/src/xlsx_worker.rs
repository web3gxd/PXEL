//! A bounded local process boundary for XLSX parsing. OS sandbox adapters are
//! only partially installed: process-tree termination is enforced, while hard
//! resource and filesystem/network isolation remain open gates.

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use spreadsheet_xlsx::worker_protocol::{
    MAX_SOURCE_BYTES, PROTOCOL_VERSION, REQUEST_MAGIC, RESPONSE_HEADER_BYTES,
};
use spreadsheet_xlsx::{NormalizedWorkbook, worker_protocol};
use std::{path::Path, process::Stdio, time::Duration};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

struct WorkerProcessGuard {
    child: Box<dyn ChildWrapper>,
    terminate_on_drop: bool,
}

impl WorkerProcessGuard {
    fn new(child: Box<dyn ChildWrapper>) -> Self {
        Self {
            child,
            terminate_on_drop: true,
        }
    }

    async fn terminate(&mut self) {
        let _ = self.child.start_kill();
        if self.child.wait().await.is_ok() {
            self.terminate_on_drop = false;
        }
    }
}

impl Drop for WorkerProcessGuard {
    fn drop(&mut self) {
        if self.terminate_on_drop {
            let _ = self.child.start_kill();
        }
    }
}

/// Parent-side entry point. Only bounded package bytes cross the boundary.
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
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| WorkerError::InvalidInput)?;
    let metadata = file
        .metadata()
        .await
        .map_err(|_| WorkerError::InvalidInput)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(WorkerError::InvalidInput);
    }
    let mut source = Vec::new();
    tokio::time::timeout(
        WORKER_TIMEOUT,
        file.take((MAX_SOURCE_BYTES + 1) as u64)
            .read_to_end(&mut source),
    )
    .await
    .map_err(|_| WorkerError::Timeout)?
    .map_err(|_| WorkerError::InvalidInput)?;
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return Err(WorkerError::InvalidInput);
    }
    let source_len = u32::try_from(source.len()).map_err(|_| WorkerError::InvalidInput)?;
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
    let mut command = CommandWrap::from(command);
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(JobObject);
    let mut child = WorkerProcessGuard::new(command.spawn().map_err(|_| WorkerError::Process)?);
    let mut stdin = child.child.stdin().take().ok_or(WorkerError::Process)?;
    let mut stdout = child.child.stdout().take().ok_or(WorkerError::Process)?;
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
            .write_all(&source_len.to_be_bytes())
            .await
            .map_err(|_| WorkerError::Protocol)?;
        stdin
            .write_all(&source)
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
        let status = child.child.wait().await.map_err(|_| WorkerError::Process)?;
        if !status.success() {
            return Err(WorkerError::Process);
        }
        child.terminate_on_drop = false;
        decode_payload(&payload)
    })
    .await;
    match transfer {
        Ok(Ok(counts)) => Ok(counts),
        Ok(Err(error)) => {
            child.terminate().await;
            Err(error)
        }
        Err(_) => {
            child.terminate().await;
            Err(WorkerError::Timeout)
        }
    }
}

impl From<worker_protocol::ProtocolError> for WorkerError {
    fn from(error: worker_protocol::ProtocolError) -> Self {
        match error {
            worker_protocol::ProtocolError::InvalidInput => Self::InvalidInput,
            worker_protocol::ProtocolError::Rejected => Self::Rejected,
            worker_protocol::ProtocolError::Protocol => Self::Protocol,
        }
    }
}

/// Native and WASI children share the exact bounded protocol implementation.
pub fn run() -> Result<(), WorkerError> {
    worker_protocol::run(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())
        .map_err(Into::into)
}
#[cfg(test)]
fn read_request(input: &mut impl std::io::Read) -> Result<Vec<u8>, WorkerError> {
    worker_protocol::read_request(input).map_err(Into::into)
}
#[cfg(test)]
fn encode_header(status: u8, length: usize) -> Result<[u8; RESPONSE_HEADER_BYTES], WorkerError> {
    worker_protocol::encode_header(status, length).map_err(Into::into)
}
fn decode_header(frame: &[u8; RESPONSE_HEADER_BYTES]) -> Result<usize, WorkerError> {
    worker_protocol::decode_header(frame).map_err(Into::into)
}
fn decode_payload(payload: &[u8]) -> Result<NormalizedWorkbook, WorkerError> {
    worker_protocol::decode_payload(payload).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    #[test]
    fn byte_request_rejects_old_version_lengths_truncation_and_trailing_bytes() {
        let mut frame = b"PXIW\0\x05\0\0\0\x03abc".to_vec();
        assert_eq!(
            super::read_request(&mut frame.as_slice()),
            Ok(b"abc".to_vec())
        );
        frame[5] = 4;
        assert_eq!(
            super::read_request(&mut frame.as_slice()),
            Err(super::WorkerError::Protocol)
        );
        assert_eq!(
            super::read_request(&mut b"PXIW\0\x05\0\0\0\0".as_slice()),
            Err(super::WorkerError::InvalidInput)
        );
        assert_eq!(
            super::read_request(&mut b"PXIW\0\x05\x02\0\0\x01".as_slice()),
            Err(super::WorkerError::InvalidInput)
        );
        assert_eq!(
            super::read_request(&mut b"PXIW\0\x05\0\0\0\x03ab".as_slice()),
            Err(super::WorkerError::Protocol)
        );
        assert_eq!(
            super::read_request(&mut b"PXIW\0\x05\0\0\0\x03abcd".as_slice()),
            Err(super::WorkerError::Protocol)
        );
    }
    use super::{WorkerError, decode_header, decode_payload, encode_header};
    #[cfg(windows)]
    use process_wrap::tokio::JobObject;
    #[cfg(unix)]
    use process_wrap::tokio::ProcessGroup;
    use process_wrap::tokio::{CommandWrap, KillOnDrop};
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::Duration,
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, BufReader},
        process::ChildStdout,
    };

    #[test]
    fn response_codec_rejects_unknown_version_status_and_size() {
        let frame = encode_header(0, 5).unwrap_or([0; 11]);
        assert_eq!(decode_header(&frame), Ok(5));
        let mut unknown = frame;
        unknown[5] = 6;
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
        let valid = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[{"row":1,"column":1,"value":{"Text":"hello"}}]}],"issues":[]}"#;
        assert!(decode_payload(valid).is_ok());
        let previous_schema =
            br#"{"schema_version":2,"sheets":[{"name":"Data","cells":[]}],"issues":[]}"#;
        assert_eq!(decode_payload(previous_schema), Err(WorkerError::Protocol));
        let duplicate = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[{"row":1,"column":1,"value":{"Text":"a"}},{"row":1,"column":1,"value":{"Text":"b"}}]}],"issues":[]}"#;
        assert_eq!(decode_payload(duplicate), Err(WorkerError::Protocol));
        let invalid_number = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[{"row":1,"column":1,"value":{"Number":-0.0}}]}],"issues":[]}"#;
        assert_eq!(decode_payload(invalid_number), Err(WorkerError::Protocol));
        let unknown = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[],"hidden":true}],"issues":[]}"#;
        assert_eq!(decode_payload(unknown), Err(WorkerError::Protocol));
        let duplicate_names =
            br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[]},{"name":"data","cells":[]}],"issues":[]}"#;
        assert_eq!(decode_payload(duplicate_names), Err(WorkerError::Protocol));
        let invented_code = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"Unknown","sheet_index":0}]}"#;
        assert_eq!(decode_payload(invented_code), Err(WorkerError::Protocol));
        let invalid_sheet = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"CellFormatting","sheet_index":1}]}"#;
        assert_eq!(decode_payload(invalid_sheet), Err(WorkerError::Protocol));
        let duplicated_issue = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"CellFormatting","sheet_index":0},{"code":"CellFormatting","sheet_index":0}]}"#;
        assert_eq!(decode_payload(duplicated_issue), Err(WorkerError::Protocol));
        let mislabeled_issue = br#"{"schema_version":3,"sheets":[{"name":"Data","cells":[]}],"issues":[{"code":"WorkbookMetadata","sheet_index":0}]}"#;
        assert_eq!(decode_payload(mislabeled_issue), Err(WorkerError::Protocol));
    }

    fn start_process_tree_probe()
    -> Result<(super::WorkerProcessGuard, BufReader<ChildStdout>), Box<dyn std::error::Error>> {
        let mut command = CommandWrap::with_new(std::env::current_exe()?, |command| {
            command
                .args([
                    "--exact",
                    "xlsx_worker::tests::process_tree_probe",
                    "--nocapture",
                ])
                .env("PXEL_XLSX_TREE_PROBE", "root")
                .env_remove("RUST_TEST_THREADS")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
        });
        command.wrap(KillOnDrop);
        #[cfg(unix)]
        command.wrap(ProcessGroup::leader());
        #[cfg(windows)]
        command.wrap(JobObject);
        let mut child = super::WorkerProcessGuard::new(command.spawn()?);
        let stdout = child
            .child
            .stdout()
            .take()
            .ok_or("worker stdout unavailable")?;
        Ok((child, BufReader::new(stdout)))
    }

    async fn wait_for_process_tree_probe(
        reader: &mut BufReader<ChildStdout>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                line.clear();
                if reader.read_line(&mut line).await? == 0 {
                    return Err(std::io::Error::other("worker exited before spawning child"));
                }
                if line.trim_end() == "PXEL_TREE_READY" {
                    return Ok::<(), std::io::Error>(());
                }
            }
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn process_tree_probe() -> Result<(), Box<dyn std::error::Error>> {
        match std::env::var("PXEL_XLSX_TREE_PROBE").as_deref() {
            Ok("descendant") => {
                std::thread::sleep(Duration::from_secs(60));
                return Ok(());
            }
            Ok("root") => {
                Command::new(std::env::current_exe()?)
                    .args([
                        "--exact",
                        "xlsx_worker::tests::process_tree_probe",
                        "--nocapture",
                    ])
                    .env("PXEL_XLSX_TREE_PROBE", "descendant")
                    .env_remove("RUST_TEST_THREADS")
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::null())
                    .spawn()?;
                println!("PXEL_TREE_READY");
                std::io::stdout().flush()?;
                std::thread::sleep(Duration::from_secs(60));
                return Ok(());
            }
            _ => {}
        }

        let (mut child, mut reader) = start_process_tree_probe()?;
        wait_for_process_tree_probe(&mut reader).await?;
        child.child.start_kill()?;
        let killed = child.child.wait().await?;
        assert!(!killed.success());
        child.terminate_on_drop = false;
        let mut remaining_output = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            reader.read_to_end(&mut remaining_output),
        )
        .await??;

        assert!(remaining_output.is_empty());

        let (child, mut reader) = start_process_tree_probe()?;
        wait_for_process_tree_probe(&mut reader).await?;
        drop(child);
        let mut remaining_output = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            reader.read_to_end(&mut remaining_output),
        )
        .await??;
        assert!(remaining_output.is_empty());
        Ok(())
    }
}
