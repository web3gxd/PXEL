//! One protocol shared by native and capability-restricted WASI executors.
use crate::NormalizedWorkbook;
use std::io::{Cursor, Read, Write};

pub const REQUEST_MAGIC: &[u8; 4] = b"PXIW";
pub const RESPONSE_MAGIC: &[u8; 4] = b"PXOW";
pub const PROTOCOL_VERSION: u16 = 5;
pub const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
pub const RESPONSE_HEADER_BYTES: usize = 11;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ProtocolError {
    #[error("XLSX worker input is invalid")]
    InvalidInput,
    #[error("XLSX worker rejected the package")]
    Rejected,
    #[error("XLSX worker protocol failed")]
    Protocol,
}

pub fn run(input: &mut impl Read, output: &mut impl Write) -> Result<(), ProtocolError> {
    let source = read_request(input)?;
    let result = crate::normalize_reader(Cursor::new(&source), source.len() as u64);
    let (status, payload) = match result {
        Ok(workbook) => {
            workbook.validate().map_err(|_| ProtocolError::Protocol)?;
            let payload = serde_json::to_vec(&workbook).map_err(|_| ProtocolError::Protocol)?;
            if payload.len() > MAX_RESPONSE_BYTES {
                return Err(ProtocolError::Rejected);
            }
            (0_u8, payload)
        }
        Err(_) => (1_u8, Vec::new()),
    };
    output
        .write_all(&encode_header(status, payload.len())?)
        .map_err(|_| ProtocolError::Protocol)?;
    output
        .write_all(&payload)
        .map_err(|_| ProtocolError::Protocol)?;
    output.flush().map_err(|_| ProtocolError::Protocol)?;
    Ok(())
}

pub fn read_request(input: &mut impl Read) -> Result<Vec<u8>, ProtocolError> {
    let mut header = [0_u8; 10];
    input
        .read_exact(&mut header)
        .map_err(|_| ProtocolError::Protocol)?;
    if &header[..4] != REQUEST_MAGIC
        || u16::from_be_bytes([header[4], header[5]]) != PROTOCOL_VERSION
    {
        return Err(ProtocolError::Protocol);
    }
    let length = u32::from_be_bytes([header[6], header[7], header[8], header[9]]) as usize;
    if length == 0 || length > MAX_SOURCE_BYTES {
        return Err(ProtocolError::InvalidInput);
    }
    let mut source = vec![0_u8; length];
    input
        .read_exact(&mut source)
        .map_err(|_| ProtocolError::Protocol)?;
    let mut extra = [0_u8; 1];
    if input
        .read(&mut extra)
        .map_err(|_| ProtocolError::Protocol)?
        != 0
    {
        return Err(ProtocolError::Protocol);
    }
    Ok(source)
}

pub fn encode_header(
    status: u8,
    length: usize,
) -> Result<[u8; RESPONSE_HEADER_BYTES], ProtocolError> {
    if length > MAX_RESPONSE_BYTES || (status == 0 && length == 0) || (status != 0 && length != 0) {
        return Err(ProtocolError::Protocol);
    }
    let mut frame = [0_u8; RESPONSE_HEADER_BYTES];
    frame[..4].copy_from_slice(RESPONSE_MAGIC);
    frame[4..6].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    frame[6] = status;
    frame[7..11].copy_from_slice(
        &u32::try_from(length)
            .map_err(|_| ProtocolError::Protocol)?
            .to_be_bytes(),
    );
    Ok(frame)
}

pub fn decode_header(frame: &[u8; RESPONSE_HEADER_BYTES]) -> Result<usize, ProtocolError> {
    if &frame[..4] != RESPONSE_MAGIC || u16::from_be_bytes([frame[4], frame[5]]) != PROTOCOL_VERSION
    {
        return Err(ProtocolError::Protocol);
    }
    if frame[6] == 1 {
        return Err(ProtocolError::Rejected);
    }
    if frame[6] != 0 {
        return Err(ProtocolError::Protocol);
    }
    let length = u32::from_be_bytes([frame[7], frame[8], frame[9], frame[10]]) as usize;
    if length == 0 || length > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::Protocol);
    }
    Ok(length)
}

pub fn decode_payload(payload: &[u8]) -> Result<NormalizedWorkbook, ProtocolError> {
    if payload.is_empty() || payload.len() > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::Protocol);
    }
    let workbook: NormalizedWorkbook =
        serde_json::from_slice(payload).map_err(|_| ProtocolError::Protocol)?;
    workbook.validate().map_err(|_| ProtocolError::Protocol)?;
    Ok(workbook)
}
