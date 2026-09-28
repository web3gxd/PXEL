//! One bounded, cancellable calculation response per WebSocket connection.
use crate::{AppState, auth};
use prost::Message;
use spreadsheet_collab::{
    DocumentSession, SessionDirectory,
    calculation::{COMPATIBILITY_PROFILE, CalculatedState, CalculationOutcome, ENGINE_VERSION},
};
use spreadsheet_domain::workbook::{CalculationContext, CellAddress, DocumentId, DocumentVersion};
use spreadsheet_formula::{FormulaError, Value};
use spreadsheet_wire_protocol::{PROTOCOL_VERSION, v1};
use std::{ops::Bound, sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

const FRAME_BYTES: usize = 256 * 1024;
pub struct Frame {
    pub bytes: Vec<u8>,
    pub session: DocumentSession,
    pub version: DocumentVersion,
}
pub struct Stream {
    receiver: mpsc::Receiver<Frame>,
    task: JoinHandle<()>,
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn next(stream: &mut Option<Stream>) -> Option<Frame> {
    match stream {
        Some(stream) => stream.receiver.recv().await,
        None => std::future::pending().await,
    }
}

pub async fn start(
    state: Arc<AppState>,
    principal: auth::Principal,
    request: v1::RequestCalculation,
) -> Result<Stream, ()> {
    let document =
        DocumentId::from_bytes(request.document_id.as_slice().try_into().map_err(|_| ())?)
            .map_err(|_| ())?;
    let version = DocumentVersion::new(request.input_version).map_err(|_| ())?;
    if version == DocumentVersion::ZERO {
        return Err(());
    }
    let session = state
        .sessions
        .session(principal.identity, document)
        .await
        .map_err(|_| ())?;
    let mut updates = session
        .watch_calculation(principal.identity)
        .await
        .map_err(|_| ())?;
    let requested_context = session
        .read(principal.identity)
        .await
        .map_err(|_| ())?
        .calculation_context()
        .cloned();
    let (sender, receiver) = mpsc::channel(2);
    let task = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let result = async {
            let calculated = loop {
                let latest = updates.borrow_and_update().clone();
                if let Some(value) = latest {
                    if value.input_version == version {
                        break value;
                    }
                    if value.input_version > version {
                        return;
                    }
                }
                if updates.changed().await.is_err() {
                    return;
                }
            };
            let mut cursor = None;
            let mut sequence = 0;
            loop {
                let Ok(Some(current)) = session.calculated(principal.identity).await else {
                    return;
                };
                if current.input_version != version
                    || std::time::Instant::now() >= principal.expires_at
                {
                    return;
                }
                let Ok(permit) = state.calculation_encoders.clone().acquire_owned().await else {
                    return;
                };
                let source = calculated.clone();
                let encoded = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    encode_chunk(&source, cursor, sequence)
                })
                .await;
                let (bytes, last, done) = match encoded {
                    Ok(Ok(chunk)) => chunk,
                    _ => (
                        failure(document, version, calculated.context.as_ref(), 4),
                        None,
                        true,
                    ),
                };
                if sender
                    .send(Frame {
                        bytes,
                        session: session.clone(),
                        version,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                if done {
                    return;
                }
                cursor = last;
                sequence += 1;
            }
        };
        // A stalled client or calculation never retains a response task indefinitely.
        if tokio::time::timeout_at(deadline, result).await.is_err() {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                sender.send(Frame {
                    bytes: failure(document, version, requested_context.as_ref(), 5),
                    session: session.clone(),
                    version,
                }),
            )
            .await;
        }
    });
    Ok(Stream { receiver, task })
}

fn context_identity(document: DocumentId, context: Option<&CalculationContext>) -> (u64, Vec<u8>) {
    context.map_or_else(
        || (0, document.as_bytes().to_vec()),
        |context| (context.epoch(), context.context_id().as_bytes().to_vec()),
    )
}

fn failure(
    document: DocumentId,
    version: DocumentVersion,
    context: Option<&CalculationContext>,
    code: i32,
) -> Vec<u8> {
    let (calculation_epoch, context_id) = context_identity(document, context);
    v1::ServerEnvelope {
        protocol_version: PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::CalculationCommitted(
            v1::CalculationCommitted {
                document_id: document.as_bytes().to_vec(),
                input_version: version.get(),
                calculation_epoch,
                formula_engine_version: ENGINE_VERSION.into(),
                compatibility_profile: COMPATIBILITY_PROFILE.into(),
                context_id,
                patch_sequence: 0,
                is_final: true,
                calculated_state_hash: Vec::new(),
                cells: Vec::new(),
                full_snapshot: true,
                error_code: code,
            },
        )),
    }
    .encode_to_vec()
}

fn value(value: &Value) -> Result<v1::CalculatedValue, ()> {
    use v1::calculated_value::Kind;
    let kind = match value {
        Value::Blank => Kind::Blank(true),
        Value::Number(number) if number.is_finite() => Kind::Number(*number),
        Value::Text(text) => Kind::Text(text.clone()),
        Value::Boolean(boolean) => Kind::Boolean(*boolean),
        Value::Error(error) => Kind::Error(match error {
            FormulaError::Null => 1,
            FormulaError::Div0 => 2,
            FormulaError::Value => 3,
            FormulaError::Ref => 4,
            FormulaError::Name => 5,
            FormulaError::Num => 6,
            FormulaError::Na => 7,
            FormulaError::Spill => 8,
            FormulaError::Cycle => 9,
        }),
        _ => return Err(()),
    };
    Ok(v1::CalculatedValue { kind: Some(kind) })
}

fn encode_chunk(
    state: &CalculatedState,
    after: Option<CellAddress>,
    sequence: u32,
) -> Result<(Vec<u8>, Option<CellAddress>, bool), ()> {
    let (calculation_epoch, context_id) =
        context_identity(state.document_id, state.context.as_ref());
    let mut message = v1::CalculationCommitted {
        document_id: state.document_id.as_bytes().to_vec(),
        input_version: state.input_version.get(),
        calculation_epoch,
        formula_engine_version: ENGINE_VERSION.into(),
        compatibility_profile: COMPATIBILITY_PROFILE.into(),
        context_id,
        patch_sequence: sequence,
        is_final: false,
        calculated_state_hash: Vec::new(),
        cells: Vec::new(),
        full_snapshot: true,
        error_code: 0,
    };
    let mut cursor = after;
    let mut size = message.encoded_len() + 64;
    let mut done = true;
    match &state.outcome {
        CalculationOutcome::Complete { results, hash, .. } => {
            let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
            for (address, result) in results.range((lower, Bound::Unbounded)) {
                let cell = v1::CalculatedCell {
                    address: Some(v1::CellAddress {
                        sheet_id: address.sheet.as_bytes().to_vec(),
                        row_id: address.row.as_bytes().to_vec(),
                        column_id: address.column.as_bytes().to_vec(),
                    }),
                    value: Some(value(result)?),
                };
                let bytes = cell.encoded_len() + 6;
                if size + bytes > FRAME_BYTES {
                    if message.cells.is_empty() {
                        return Err(());
                    }
                    done = false;
                    break;
                }
                size += bytes;
                cursor = Some(*address);
                message.cells.push(cell);
            }
            if done {
                message.calculated_state_hash = hash.to_vec();
            }
        }
        CalculationOutcome::Failed(error) => {
            use spreadsheet_formula::{EvaluationError, calculation::CalculationError};
            message.error_code = match error {
                CalculationError::DependencyLimit
                | CalculationError::Evaluation(EvaluationError::CalculationLimitExceeded) => 1,
                CalculationError::Evaluation(EvaluationError::Cancelled) => 2,
                _ => 4,
            };
        }
    }
    message.is_final = done;
    let bytes = v1::ServerEnvelope {
        protocol_version: PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::CalculationCommitted(message)),
    }
    .encode_to_vec();
    if bytes.len() > FRAME_BYTES {
        return Err(());
    }
    Ok((bytes, cursor, done))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::workbook::{ColumnId, RowId, SheetId};
    use std::collections::BTreeMap;

    #[test]
    fn chunked_results_are_bounded_complete_and_hash_only_final()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut results = BTreeMap::new();
        for index in 1_u128..=600 {
            results.insert(
                CellAddress {
                    sheet: SheetId::from_bytes([1; 16])?,
                    row: RowId::from_bytes(index.to_be_bytes())?,
                    column: ColumnId::from_bytes([2; 16])?,
                },
                Value::Text("x".repeat(1024)),
            );
        }
        let state = CalculatedState {
            document_id: DocumentId::from_bytes([3; 16])?,
            input_version: DocumentVersion::new(12)?,
            context: None,
            outcome: CalculationOutcome::Complete {
                results,
                hash: [5; 32],
                evaluated_cells: 600,
            },
        };
        let mut after = None;
        let mut count = 0;
        let mut sequence = 0;
        loop {
            let (bytes, cursor, done) =
                encode_chunk(&state, after, sequence).map_err(|()| "encode failed")?;
            assert!(bytes.len() <= FRAME_BYTES);
            let envelope = v1::ServerEnvelope::decode(bytes.as_slice())?;
            let Some(v1::server_envelope::Payload::CalculationCommitted(chunk)) = envelope.payload
            else {
                return Err("wrong payload".into());
            };
            assert_eq!(chunk.patch_sequence, sequence);
            assert_eq!(chunk.input_version, 12);
            assert_eq!(chunk.is_final, done);
            assert!(chunk.full_snapshot);
            assert_eq!(
                chunk.calculated_state_hash,
                if done { vec![5; 32] } else { vec![] }
            );
            for cell in chunk.cells {
                let row = cell.address.ok_or("missing address")?.row_id;
                count += 1;
                assert_eq!(row, (count as u128).to_be_bytes());
            }
            if done {
                break;
            }
            assert_ne!(cursor, after);
            after = cursor;
            sequence += 1;
        }
        assert_eq!(count, 600);
        assert!(sequence >= 2);
        Ok(())
    }
}
