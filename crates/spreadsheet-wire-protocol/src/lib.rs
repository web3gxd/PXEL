//! Connection-only wire contract. Persisted event codecs live separately.
pub mod formula;
pub mod style;
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/pxel.wire.v1.rs"));
}

use prost::Message;
use thiserror::Error;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const MAX_HELLO_BYTES: usize = 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    #[error("invalid handshake message")]
    InvalidMessage,
    #[error("message exceeds limit")]
    TooLarge,
}

pub fn decode_hello(bytes: &[u8]) -> Result<v1::ClientHello, DecodeError> {
    if bytes.len() > MAX_HELLO_BYTES {
        return Err(DecodeError::TooLarge);
    }
    let envelope = v1::ClientEnvelope::decode(bytes).map_err(|_| DecodeError::InvalidMessage)?;
    if envelope.protocol_version != PROTOCOL_VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }
    let Some(v1::client_envelope::Payload::Hello(hello)) = envelope.payload else {
        return Err(DecodeError::InvalidMessage);
    };
    if hello.client_instance_id.len() != 16
        || hello.client_instance_id.iter().all(|byte| *byte == 0)
        || hello.client_name.is_empty()
        || hello.client_name.len() > 64
        || hello.client_name.chars().any(char::is_control)
    {
        return Err(DecodeError::InvalidMessage);
    }
    Ok(hello)
}

pub fn decode_command(bytes: &[u8]) -> Result<v1::DocumentCommand, DecodeError> {
    match decode_request(bytes)? {
        ClientRequest::Command(command) => Ok(*command),
        ClientRequest::Calculation(_) => Err(DecodeError::InvalidMessage),
        ClientRequest::Subscribe(_) => Err(DecodeError::InvalidMessage),
        ClientRequest::Presence(_) => Err(DecodeError::InvalidMessage),
    }
}

#[derive(Debug)]
pub enum ClientRequest {
    Command(Box<v1::DocumentCommand>),
    Calculation(v1::RequestCalculation),
    Subscribe(v1::SubscribeDocument),
    Presence(v1::SetPresence),
}

pub fn decode_request(bytes: &[u8]) -> Result<ClientRequest, DecodeError> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(DecodeError::TooLarge);
    }
    let envelope = v1::ClientEnvelope::decode(bytes).map_err(|_| DecodeError::InvalidMessage)?;
    if envelope.protocol_version != PROTOCOL_VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }
    match envelope.payload {
        Some(v1::client_envelope::Payload::Command(command)) => Ok(ClientRequest::Command(command)),
        Some(v1::client_envelope::Payload::RequestCalculation(request)) => {
            if request.document_id.len() != 16
                || request.document_id.iter().all(|byte| *byte == 0)
                || request.input_version == 0
                || request.input_version > i64::MAX as u64
                || request
                    .base_input_version
                    .is_some_and(|version| version > i64::MAX as u64)
            {
                return Err(DecodeError::InvalidMessage);
            }
            Ok(ClientRequest::Calculation(request))
        }
        Some(v1::client_envelope::Payload::SubscribeDocument(request)) => {
            if request.document_id.len() != 16
                || request.document_id.iter().all(|byte| *byte == 0)
                || request.last_seen_version > i64::MAX as u64
            {
                return Err(DecodeError::InvalidMessage);
            }
            Ok(ClientRequest::Subscribe(request))
        }
        Some(v1::client_envelope::Payload::SetPresence(request)) => {
            let valid_address = |address: &v1::CellAddress| {
                address.sheet_id.len() == 16
                    && address.row_id.len() == 16
                    && address.column_id.len() == 16
                    && !address.sheet_id.iter().all(|byte| *byte == 0)
                    && !address.row_id.iter().all(|byte| *byte == 0)
                    && !address.column_id.iter().all(|byte| *byte == 0)
            };
            if request.document_id.len() != 16
                || request.document_id.iter().all(|byte| *byte == 0)
                || request
                    .anchor
                    .as_ref()
                    .is_some_and(|address| !valid_address(address))
                || request
                    .focus
                    .as_ref()
                    .is_some_and(|address| !valid_address(address))
                || request.anchor.is_some() != request.focus.is_some()
                || request
                    .anchor
                    .as_ref()
                    .zip(request.focus.as_ref())
                    .is_some_and(|(anchor, focus)| anchor.sheet_id != focus.sheet_id)
            {
                return Err(DecodeError::InvalidMessage);
            }
            Ok(ClientRequest::Presence(request))
        }
        _ => Err(DecodeError::InvalidMessage),
    }
}

pub fn error_message(error: DecodeError) -> Vec<u8> {
    let code = match error {
        DecodeError::UnsupportedVersion => v1::ErrorCode::UnsupportedProtocol,
        DecodeError::InvalidMessage | DecodeError::TooLarge => v1::ErrorCode::InvalidMessage,
    };
    v1::ServerEnvelope {
        protocol_version: PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::Error(v1::ProtocolError {
            code: code.into(),
            supported_protocol_version: PROTOCOL_VERSION,
        })),
    }
    .encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_requires_complete_same_sheet_addresses() {
        let address = v1::CellAddress {
            sheet_id: vec![1; 16],
            row_id: vec![2; 16],
            column_id: vec![3; 16],
        };
        let make = |anchor: Option<v1::CellAddress>, focus: Option<v1::CellAddress>| {
            v1::ClientEnvelope {
                protocol_version: PROTOCOL_VERSION,
                payload: Some(v1::client_envelope::Payload::SetPresence(v1::SetPresence {
                    document_id: vec![4; 16],
                    anchor,
                    focus,
                })),
            }
            .encode_to_vec()
        };
        assert!(matches!(
            decode_request(&make(Some(address.clone()), Some(address.clone()))),
            Ok(ClientRequest::Presence(_))
        ));
        assert!(matches!(
            decode_request(&make(None, None)),
            Ok(ClientRequest::Presence(_))
        ));
        assert_eq!(
            decode_request(&make(Some(address.clone()), None)).err(),
            Some(DecodeError::InvalidMessage)
        );
        let mut other = address.clone();
        other.sheet_id = vec![5; 16];
        assert_eq!(
            decode_request(&make(Some(address), Some(other))).err(),
            Some(DecodeError::InvalidMessage)
        );
    }

    fn hello(version: u32) -> Vec<u8> {
        v1::ClientEnvelope {
            protocol_version: version,
            payload: Some(v1::client_envelope::Payload::Hello(v1::ClientHello {
                client_instance_id: vec![1; 16],
                client_name: "pxel-web".into(),
            })),
        }
        .encode_to_vec()
    }

    #[test]
    fn validates_version_before_accepting_payload() {
        assert!(decode_hello(&hello(1)).is_ok());
        assert_eq!(
            decode_hello(&hello(2)),
            Err(DecodeError::UnsupportedVersion)
        );
    }

    #[test]
    fn rejects_unknown_payload_and_malformed_or_oversized_data() {
        assert_eq!(
            decode_hello(&[8, 1, 26, 0]),
            Err(DecodeError::InvalidMessage)
        );
        assert_eq!(decode_hello(&[255]), Err(DecodeError::InvalidMessage));
        assert_eq!(
            decode_hello(&vec![0; MAX_HELLO_BYTES + 1]),
            Err(DecodeError::TooLarge)
        );
    }

    #[test]
    fn shared_typescript_fixture_decodes() -> Result<(), Box<dyn std::error::Error>> {
        // Canonical fixture: protocol v1, UUID bytes 01 repeated, client_name pxel-web.
        let expected: Vec<u8> = [
            vec![8, 1, 18, 28, 10, 16],
            vec![1; 16],
            vec![18, 8],
            b"pxel-web".to_vec(),
        ]
        .concat();
        assert_eq!(hello(1), expected);
        assert_eq!(decode_hello(&expected)?.client_name, "pxel-web");
        Ok(())
    }

    #[test]
    fn calculation_request_has_independent_validation() -> Result<(), Box<dyn std::error::Error>> {
        let mut request = v1::ClientEnvelope {
            protocol_version: PROTOCOL_VERSION,
            payload: Some(v1::client_envelope::Payload::RequestCalculation(
                v1::RequestCalculation {
                    document_id: vec![1; 16],
                    input_version: 2,
                    base_input_version: None,
                },
            )),
        };
        assert!(matches!(
            decode_request(&request.encode_to_vec())?,
            ClientRequest::Calculation(_)
        ));
        assert_eq!(
            decode_command(&request.encode_to_vec()),
            Err(DecodeError::InvalidMessage)
        );
        if let Some(v1::client_envelope::Payload::RequestCalculation(value)) = &mut request.payload
        {
            value.base_input_version = Some(1);
        }
        assert!(matches!(
            decode_request(&request.encode_to_vec())?,
            ClientRequest::Calculation(_)
        ));
        if let Some(v1::client_envelope::Payload::RequestCalculation(value)) = &mut request.payload
        {
            value.base_input_version = Some(i64::MAX as u64 + 1);
        }
        assert!(matches!(
            decode_request(&request.encode_to_vec()),
            Err(DecodeError::InvalidMessage)
        ));
        if let Some(v1::client_envelope::Payload::RequestCalculation(value)) = &mut request.payload
        {
            value.base_input_version = None;
            value.input_version = 0;
        }
        assert!(matches!(
            decode_request(&request.encode_to_vec()),
            Err(DecodeError::InvalidMessage)
        ));
        if let Some(v1::client_envelope::Payload::RequestCalculation(value)) = &mut request.payload
        {
            value.input_version = 2;
            value.document_id = vec![0; 16];
        }
        assert!(matches!(
            decode_request(&request.encode_to_vec()),
            Err(DecodeError::InvalidMessage)
        ));
        request.protocol_version = 99;
        assert!(matches!(
            decode_request(&request.encode_to_vec()),
            Err(DecodeError::UnsupportedVersion)
        ));
        Ok(())
    }

    #[test]
    fn subscription_rejects_future_overflow_and_empty_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut request = v1::ClientEnvelope {
            protocol_version: PROTOCOL_VERSION,
            payload: Some(v1::client_envelope::Payload::SubscribeDocument(
                v1::SubscribeDocument {
                    document_id: vec![1; 16],
                    last_seen_version: 0,
                },
            )),
        };
        assert!(matches!(
            decode_request(&request.encode_to_vec())?,
            ClientRequest::Subscribe(_)
        ));
        assert_eq!(
            decode_command(&request.encode_to_vec()),
            Err(DecodeError::InvalidMessage)
        );
        if let Some(v1::client_envelope::Payload::SubscribeDocument(value)) = &mut request.payload {
            value.last_seen_version = i64::MAX as u64 + 1;
        }
        assert!(matches!(
            decode_request(&request.encode_to_vec()),
            Err(DecodeError::InvalidMessage)
        ));
        if let Some(v1::client_envelope::Payload::SubscribeDocument(value)) = &mut request.payload {
            value.last_seen_version = 1;
            value.document_id = vec![0; 16];
        }
        assert!(matches!(
            decode_request(&request.encode_to_vec()),
            Err(DecodeError::InvalidMessage)
        ));
        Ok(())
    }
}
