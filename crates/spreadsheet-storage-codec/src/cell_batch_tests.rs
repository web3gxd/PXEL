use super::*;
use spreadsheet_domain::{
    style::{Change, HorizontalAlignment, StyleChange},
    style_write::GuardedStyleWrite,
};

fn address() -> Result<CellAddress, CodecError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes([4; 16]).map_err(|_| CodecError::Invalid)?,
        row: RowId::from_bytes([5; 16]).map_err(|_| CodecError::Invalid)?,
        column: ColumnId::from_bytes([6; 16]).map_err(|_| CodecError::Invalid)?,
    })
}

fn event(operation: Operation, version: u64) -> Result<CommittedEvent, CodecError> {
    Ok(CommittedEvent {
        document_id: DocumentId::from_bytes([1; 16]).map_err(|_| CodecError::Invalid)?,
        operation_id: OperationId::from_bytes([2; 16]).map_err(|_| CodecError::Invalid)?,
        principal: EventPrincipal::User(
            ActorId::from_bytes([3; 16]).map_err(|_| CodecError::Invalid)?,
        ),
        version: DocumentVersion::new(version).map_err(|_| CodecError::Invalid)?,
        operation,
        state_hash: [7; 32],
    })
}

fn authored() -> Result<Operation, CodecError> {
    let address = address()?;
    Ok(Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::SetValues,
        content_writes: vec![GuardedContentWrite {
            address,
            content: Some(CellContent::Value(CellValue::Text(
                "многострочно\nзначение".into(),
            ))),
            previous: None,
        }],
        style_writes: vec![GuardedStyleWrite {
            address,
            previous: vec![StyleChange::Horizontal(Change::Reset)],
            changes: vec![StyleChange::Horizontal(Change::Set(
                HorizontalAlignment::Center,
            ))],
        }],
        undoes: None,
        undoes_version: None,
    })
}

#[test]
fn cell_batch_schemas_roundtrip_values_styles_and_inverse_identity() -> Result<(), CodecError> {
    let original = event(authored()?, 2)?;
    let bytes = encode(&original)?;
    assert_eq!(
        v1::EventEnvelope::decode(bytes.as_slice())
            .map_err(|_| CodecError::Invalid)?
            .schema_version,
        28
    );
    let decoded = decode(&bytes)?;
    assert_eq!(decoded, original);
    assert_eq!(event_hash(&decoded)?, event_hash(&original)?);

    let Operation::CellBatchWrite {
        content_writes,
        style_writes,
        ..
    } = original.operation
    else {
        return Err(CodecError::Invalid);
    };
    let inverse = Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::SetValues,
        content_writes: content_writes
            .into_iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous,
                previous: write.content,
            })
            .collect(),
        style_writes: style_writes
            .into_iter()
            .map(|write| GuardedStyleWrite {
                address: write.address,
                previous: write.changes,
                changes: write.previous,
            })
            .collect(),
        undoes: Some(OperationId::from_bytes([2; 16]).map_err(|_| CodecError::Invalid)?),
        undoes_version: Some(DocumentVersion::new(2).map_err(|_| CodecError::Invalid)?),
    };
    let compensated = event(inverse, 3)?;
    let encoded = encode(&compensated)?;
    assert_eq!(
        v1::EventEnvelope::decode(encoded.as_slice())
            .map_err(|_| CodecError::Invalid)?
            .schema_version,
        29
    );
    assert_eq!(decode(&encoded)?, compensated);
    assert_eq!(event_hash(&decode(&encoded)?)?, event_hash(&compensated)?);
    Ok(())
}

#[test]
fn cell_batch_rejects_styles_outside_content_footprint() -> Result<(), CodecError> {
    let mut operation = authored()?;
    let Operation::CellBatchWrite { style_writes, .. } = &mut operation else {
        return Err(CodecError::Invalid);
    };
    style_writes[0].address.row = RowId::from_bytes([9; 16]).map_err(|_| CodecError::Invalid)?;
    assert!(matches!(
        encode(&event(operation, 2)?),
        Err(CodecError::Invalid)
    ));
    Ok(())
}

#[test]
fn styled_copy_uses_additive_schema_and_preserves_geometry_through_undo() -> Result<(), CodecError>
{
    let destination = address()?;
    let source = CellAddress {
        row: RowId::from_bytes([8; 16]).map_err(|_| CodecError::Invalid)?,
        ..destination
    };
    let operation = Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::CopyRange {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
        content_writes: vec![GuardedContentWrite {
            address: destination,
            content: Some(CellContent::Value(CellValue::Text("copied".into()))),
            previous: None,
        }],
        style_writes: authored_style(destination),
        undoes: None,
        undoes_version: None,
    };
    let original = event(operation, 2)?;
    let bytes = encode(&original)?;
    let decoded_envelope =
        v1::EventEnvelope::decode(bytes.as_slice()).map_err(|_| CodecError::Invalid)?;
    assert_eq!(decoded_envelope.schema_version, 30);
    let decoded = decode(&bytes)?;
    assert_eq!(decoded, original);
    assert_eq!(event_hash(&decoded)?, event_hash(&original)?);

    let Operation::CellBatchWrite {
        intent,
        content_writes,
        style_writes,
        ..
    } = original.operation
    else {
        return Err(CodecError::Invalid);
    };
    let inverse = Operation::CellBatchWrite {
        intent,
        content_writes: content_writes
            .into_iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous,
                previous: write.content,
            })
            .collect(),
        style_writes: style_writes
            .into_iter()
            .map(|write| GuardedStyleWrite {
                address: write.address,
                previous: write.changes,
                changes: write.previous,
            })
            .collect(),
        undoes: Some(OperationId::from_bytes([2; 16]).map_err(|_| CodecError::Invalid)?),
        undoes_version: Some(DocumentVersion::new(2).map_err(|_| CodecError::Invalid)?),
    };
    let inverse_event = event(inverse, 3)?;
    let inverse_bytes = encode(&inverse_event)?;
    assert_eq!(
        v1::EventEnvelope::decode(inverse_bytes.as_slice())
            .map_err(|_| CodecError::Invalid)?
            .schema_version,
        31
    );
    assert_eq!(decode(&inverse_bytes)?, inverse_event);
    assert_eq!(
        event_hash(&decode(&inverse_bytes)?)?,
        event_hash(&inverse_event)?
    );
    Ok(())
}

#[test]
fn styled_move_uses_new_schemas_and_hashes_geometry() -> Result<(), CodecError> {
    let destination = address()?;
    let source = CellAddress {
        row: RowId::from_bytes([8; 16]).map_err(|_| CodecError::Invalid)?,
        ..destination
    };
    let make = |undoes, undoes_version| Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::MoveRange {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
        content_writes: vec![GuardedContentWrite {
            address: destination,
            content: None,
            previous: Some(CellContent::Value(CellValue::Text("moved".into()))),
        }],
        style_writes: authored_style(destination),
        undoes,
        undoes_version,
    };
    let authored = event(make(None, None), 2)?;
    let bytes = encode(&authored)?;
    assert_eq!(
        v1::EventEnvelope::decode(bytes.as_slice())
            .map_err(|_| CodecError::Invalid)?
            .schema_version,
        32
    );
    let decoded = decode(&bytes)?;
    assert_eq!(decoded, authored);
    assert_eq!(event_hash(&decoded)?, event_hash(&authored)?);
    let inverse = event(
        make(
            Some(OperationId::from_bytes([2; 16]).map_err(|_| CodecError::Invalid)?),
            Some(DocumentVersion::new(2).map_err(|_| CodecError::Invalid)?),
        ),
        3,
    )?;
    let inverse_bytes = encode(&inverse)?;
    assert_eq!(
        v1::EventEnvelope::decode(inverse_bytes.as_slice())
            .map_err(|_| CodecError::Invalid)?
            .schema_version,
        33
    );
    assert_eq!(decode(&inverse_bytes)?, inverse);
    assert_eq!(event_hash(&decode(&inverse_bytes)?)?, event_hash(&inverse)?);
    Ok(())
}

fn authored_style(address: CellAddress) -> Vec<GuardedStyleWrite> {
    vec![GuardedStyleWrite {
        address,
        previous: vec![StyleChange::Horizontal(Change::Reset)],
        changes: vec![StyleChange::Horizontal(Change::Set(
            HorizontalAlignment::Center,
        ))],
    }]
}
