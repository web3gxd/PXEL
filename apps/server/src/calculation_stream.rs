//! One bounded, cancellable calculation response per WebSocket connection.
use crate::{AppState, auth, delivery::DeliveryGate};
use prost::Message;
use spreadsheet_application::journal::{DocumentAccess, OperationJournal};
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
    pub document: DocumentId,
    pub gate: Arc<DeliveryGate>,
    pub ticket: DocumentAccess,
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
    let requested_base = request
        .base_input_version
        .and_then(|value| DocumentVersion::new(value).ok());
    let gate = state.delivery.get(document).await;
    let ticket = state
        .database
        .access(principal.identity, document)
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
                    encode_chunk(&source, requested_base, cursor, sequence)
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
                        document,
                        gate: gate.clone(),
                        ticket,
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
                    document,
                    gate: gate.clone(),
                    ticket,
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
                base_input_version: None,
                evaluated_cells: 0,
                dirty_cells: 0,
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
    requested_base: Option<DocumentVersion>,
    after: Option<CellAddress>,
    sequence: u32,
) -> Result<(Vec<u8>, Option<CellAddress>, bool), ()> {
    let (calculation_epoch, context_id) =
        context_identity(state.document_id, state.context.as_ref());
    let delta = matches!(&state.outcome, CalculationOutcome::Complete { .. })
        && requested_base.is_some_and(|base| {
            base < state.input_version && Some(base) == state.base_input_version
        });
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
        full_snapshot: !delta,
        error_code: 0,
        base_input_version: if delta {
            requested_base.map(DocumentVersion::get)
        } else {
            None
        },
        evaluated_cells: 0,
        dirty_cells: 0,
    };
    let mut cursor = after;
    let mut size = message.encoded_len() + 64;
    let mut done = true;
    match &state.outcome {
        CalculationOutcome::Complete {
            results,
            changed_results,
            cleared_results,
            hash,
            evaluated_cells,
            dirty_cells,
        } => {
            message.evaluated_cells = u32::try_from(*evaluated_cells).map_err(|_| ())?;
            message.dirty_cells = u32::try_from(*dirty_cells).map_err(|_| ())?;
            let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
            let mut append = |address: CellAddress, result: Option<&Value>| -> Result<bool, ()> {
                let cell = v1::CalculatedCell {
                    address: Some(v1::CellAddress {
                        sheet_id: address.sheet.as_bytes().to_vec(),
                        row_id: address.row.as_bytes().to_vec(),
                        column_id: address.column.as_bytes().to_vec(),
                    }),
                    value: result.map(value).transpose()?,
                };
                let bytes = cell.encoded_len() + 6;
                if size + bytes > FRAME_BYTES {
                    if message.cells.is_empty() {
                        return Err(());
                    }
                    return Ok(false);
                }
                size += bytes;
                cursor = Some(address);
                message.cells.push(cell);
                Ok(true)
            };
            if delta {
                let mut changes = changed_results.range((lower, Bound::Unbounded)).peekable();
                let mut clears = cleared_results.range((lower, Bound::Unbounded)).peekable();
                loop {
                    let next_change = changes.peek().map(|(address, _)| **address);
                    let next_clear = clears.peek().map(|address| **address);
                    let item = match (next_change, next_clear) {
                        (None, None) => break,
                        (Some(address), None) => {
                            let (_, result) = changes.next().ok_or(())?;
                            (address, Some(result))
                        }
                        (None, Some(address)) => {
                            clears.next().ok_or(())?;
                            (address, None)
                        }
                        (Some(change), Some(clear)) if change < clear => {
                            let (_, result) = changes.next().ok_or(())?;
                            (change, Some(result))
                        }
                        (Some(change), Some(clear)) if clear < change => {
                            clears.next().ok_or(())?;
                            (clear, None)
                        }
                        (Some(_), Some(_)) => return Err(()),
                    };
                    if !append(item.0, item.1)? {
                        done = false;
                        break;
                    }
                }
            } else {
                for (address, result) in results.range((lower, Bound::Unbounded)) {
                    if !append(*address, Some(result))? {
                        done = false;
                        break;
                    }
                }
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
    use std::collections::{BTreeMap, BTreeSet};

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
            base_input_version: None,
            outcome: CalculationOutcome::Complete {
                results,
                changed_results: BTreeMap::new(),
                cleared_results: BTreeSet::new(),
                hash: [5; 32],
                evaluated_cells: 600,
                dirty_cells: 600,
            },
        };
        let mut after = None;
        let mut count = 0;
        let mut sequence = 0;
        loop {
            let (bytes, cursor, done) =
                encode_chunk(&state, None, after, sequence).map_err(|()| "encode failed")?;
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
            assert_eq!(chunk.base_input_version, None);
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

    #[test]
    fn matching_calculation_base_sends_a_delta_and_mismatch_falls_back_to_full()
    -> Result<(), Box<dyn std::error::Error>> {
        const FORMULA_COUNT: u128 = 10_000;
        let document_id = DocumentId::from_bytes([3; 16])?;
        let sheet = SheetId::from_bytes([1; 16])?;
        let column = ColumnId::from_bytes([2; 16])?;
        let mut results = BTreeMap::new();
        let mut changed_results = BTreeMap::new();
        let mut cleared_results = BTreeSet::new();
        let changed_address = CellAddress {
            sheet,
            row: RowId::from_bytes(5_000_u128.to_be_bytes())?,
            column,
        };
        let cleared_address = CellAddress {
            sheet,
            row: RowId::from_bytes(5_001_u128.to_be_bytes())?,
            column,
        };
        for index in 1..=FORMULA_COUNT {
            let address = CellAddress {
                sheet,
                row: RowId::from_bytes(index.to_be_bytes())?,
                column,
            };
            let value = Value::Number(if index == 5_000 { 8.0 } else { 2.0 });
            if address == changed_address {
                changed_results.insert(address, value.clone());
            }
            if address == cleared_address {
                cleared_results.insert(address);
            } else {
                results.insert(address, value);
            }
        }
        let state = CalculatedState {
            document_id,
            input_version: DocumentVersion::new(12)?,
            context: None,
            base_input_version: Some(DocumentVersion::new(11)?),
            outcome: CalculationOutcome::Complete {
                results,
                changed_results,
                cleared_results,
                hash: [9; 32],
                evaluated_cells: 1,
                dirty_cells: 1,
            },
        };

        let (delta_bytes, _, delta_done) =
            encode_chunk(&state, Some(DocumentVersion::new(11)?), None, 0)
                .map_err(|()| "delta encode failed")?;
        let delta_envelope = v1::ServerEnvelope::decode(delta_bytes.as_slice())?;
        let Some(v1::server_envelope::Payload::CalculationCommitted(delta)) =
            delta_envelope.payload
        else {
            return Err("wrong delta payload".into());
        };
        assert!(delta_done);
        assert!(!delta.full_snapshot);
        assert_eq!(delta.base_input_version, Some(11));
        assert_eq!(delta.cells.len(), 2);
        assert!(delta.cells.iter().any(|cell| cell.value.is_none()));
        assert_eq!(delta.evaluated_cells, 1);
        assert_eq!(delta.dirty_cells, 1);

        let mut full_cursor = None;
        let mut full_sequence = 0;
        let mut full_cell_count = 0;
        let mut full_payload_bytes = 0;
        loop {
            let (full_bytes, cursor, done) = encode_chunk(
                &state,
                Some(DocumentVersion::new(10)?),
                full_cursor,
                full_sequence,
            )
            .map_err(|()| "fallback encode failed")?;
            assert!(full_bytes.len() <= FRAME_BYTES);
            full_payload_bytes += full_bytes.len();
            let full_envelope = v1::ServerEnvelope::decode(full_bytes.as_slice())?;
            let Some(v1::server_envelope::Payload::CalculationCommitted(full)) =
                full_envelope.payload
            else {
                return Err("wrong fallback payload".into());
            };
            assert!(full.full_snapshot);
            assert_eq!(full.base_input_version, None);
            assert_eq!(full.patch_sequence, full_sequence);
            assert_eq!(full.is_final, done);
            assert_eq!(
                full.calculated_state_hash,
                if done { vec![9; 32] } else { vec![] }
            );
            full_cell_count += full.cells.len();
            if done {
                break;
            }
            assert_ne!(cursor, full_cursor);
            full_cursor = cursor;
            full_sequence += 1;
        }
        assert_eq!(full_cell_count, 9_999);
        assert!(full_payload_bytes > delta_bytes.len() * 100);
        println!(
            "calculation wire size for 10000 formulas: delta={}B, full={}B ({:.1}x smaller)",
            delta_bytes.len(),
            full_payload_bytes,
            full_payload_bytes as f64 / delta_bytes.len() as f64
        );
        Ok(())
    }

    #[test]
    fn calculation_errors_are_full_frames_even_when_the_requested_base_matches()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = CalculatedState {
            document_id: DocumentId::from_bytes([3; 16])?,
            input_version: DocumentVersion::new(12)?,
            context: None,
            base_input_version: Some(DocumentVersion::new(11)?),
            outcome: CalculationOutcome::Failed(
                spreadsheet_formula::calculation::CalculationError::InvalidProjection,
            ),
        };
        let (bytes, _, done) = encode_chunk(&state, Some(DocumentVersion::new(11)?), None, 0)
            .map_err(|()| "error encode failed")?;
        let envelope = v1::ServerEnvelope::decode(bytes.as_slice())?;
        let Some(v1::server_envelope::Payload::CalculationCommitted(message)) = envelope.payload
        else {
            return Err("wrong error payload".into());
        };
        assert!(done);
        assert!(message.is_final);
        assert!(message.full_snapshot);
        assert_eq!(message.base_input_version, None);
        assert_eq!(message.evaluated_cells, 0);
        assert_eq!(message.dirty_cells, 0);
        assert_eq!(message.error_code, 4);
        Ok(())
    }
}
