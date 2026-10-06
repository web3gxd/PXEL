use super::*;
use spreadsheet_domain::sort::{SortKey, SortRange};

fn event(count: u32) -> Result<CommittedEvent, CodecError> {
    let source = CellAddress {
        sheet: SheetId::from_bytes([2; 16]).map_err(|_| CodecError::Invalid)?,
        row: RowId::from_bytes(1_u128.to_be_bytes()).map_err(|_| CodecError::Invalid)?,
        column: ColumnId::from_bytes([3; 16]).map_err(|_| CodecError::Invalid)?,
    };
    let order = (1..=count)
        .map(|n| RowId::from_bytes(u128::from(n).to_be_bytes()).map_err(|_| CodecError::Invalid))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CommittedEvent {
        document_id: DocumentId::from_bytes([1; 16]).map_err(|_| CodecError::Invalid)?,
        operation_id: OperationId::from_bytes([4; 16]).map_err(|_| CodecError::Invalid)?,
        principal: EventPrincipal::User(
            ActorId::from_bytes([5; 16]).map_err(|_| CodecError::Invalid)?,
        ),
        version: DocumentVersion::new(2).map_err(|_| CodecError::Invalid)?,
        state_hash: [6; 32],
        operation: Operation::CellBatchWrite {
            intent: CellBatchIntent::SortRange {
                range: SortRange {
                    source,
                    rows: count,
                    columns: 1,
                    header_rows: 0,
                    keys: vec![SortKey {
                        column: source.column,
                        descending: false,
                    }],
                },
                order: order.clone(),
            },
            content_writes: order
                .iter()
                .map(|row| GuardedContentWrite {
                    address: CellAddress {
                        row: *row,
                        ..source
                    },
                    content: None,
                    previous: None,
                })
                .collect(),
            style_writes: vec![],
            undoes: None,
            undoes_version: None,
        },
    })
}

#[test]
fn sort_has_independent_authored_inverse_families_and_rejects_downgrade() -> Result<(), CodecError>
{
    let authored = event(1001)?;
    let bytes = encode(&authored)?;
    assert_eq!(schema_version(&authored.operation), 37);
    assert_eq!(decode(&bytes)?, authored);
    assert_eq!(event_hash(&decode(&bytes)?)?, event_hash(&authored)?);
    let mut envelope =
        v1::EventEnvelope::decode(bytes.as_slice()).map_err(|_| CodecError::Invalid)?;
    envelope.schema_version = 28;
    assert!(decode(&envelope.encode_to_vec()).is_err());
    let mut inverse = authored.clone();
    inverse.version = DocumentVersion::new(3).map_err(|_| CodecError::Invalid)?;
    let Operation::CellBatchWrite {
        undoes,
        undoes_version,
        ..
    } = &mut inverse.operation
    else {
        return Err(CodecError::Invalid);
    };
    *undoes = Some(authored.operation_id);
    *undoes_version = Some(authored.version);
    assert_eq!(schema_version(&inverse.operation), 38);
    assert_eq!(decode(&encode(&inverse)?)?, inverse);
    let mut legacy = authored;
    let Operation::CellBatchWrite { intent, .. } = &mut legacy.operation else {
        return Err(CodecError::Invalid);
    };
    *intent = CellBatchIntent::SetValues;
    assert!(encode(&legacy).is_err());
    Ok(())
}

#[test]
fn selected_cell_limit_does_not_bypass_the_existing_four_mib_envelope_limit()
-> Result<(), CodecError> {
    assert!(matches!(
        encode(&event(100_000)?),
        Err(CodecError::EventTooLarge)
    ));
    assert!(event(100_001).and_then(|event| encode(&event)).is_err());
    Ok(())
}

#[test]
fn sort_rejects_duplicate_source_rows_and_styles_outside_footprint() -> Result<(), CodecError> {
    let mut invalid = event(2)?;
    let Operation::CellBatchWrite {
        intent: CellBatchIntent::SortRange { order, .. },
        ..
    } = &mut invalid.operation
    else {
        return Err(CodecError::Invalid);
    };
    order[1] = order[0];
    assert!(encode(&invalid).is_err());
    let mut invalid = event(2)?;
    let Operation::CellBatchWrite {
        style_writes,
        content_writes,
        ..
    } = &mut invalid.operation
    else {
        return Err(CodecError::Invalid);
    };
    style_writes.push(GuardedStyleWrite {
        address: CellAddress {
            row: RowId::from_bytes([99; 16]).map_err(|_| CodecError::Invalid)?,
            ..content_writes[0].address
        },
        previous: vec![spreadsheet_domain::style::StyleChange::FontBold(
            spreadsheet_domain::style::Change::Reset,
        )],
        changes: vec![spreadsheet_domain::style::StyleChange::FontBold(
            spreadsheet_domain::style::Change::Set(true),
        )],
    });
    assert!(encode(&invalid).is_err());
    Ok(())
}
