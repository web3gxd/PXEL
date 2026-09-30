use super::*;
use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn address(index: u32) -> Result<CellAddress, DomainError> {
    let mut row = [3; 16];
    row[..4].copy_from_slice(&index.to_le_bytes());
    Ok(CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: RowId::from_bytes(row)?,
        column: ColumnId::from_bytes([2; 16])?,
    })
}
fn formula(address: CellAddress, version: u64) -> Result<CellContent, Box<dyn std::error::Error>> {
    Ok(CellContent::Formula(Formula::new(
        "=42".into(),
        "=42".into(),
        BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(42.)])?,
        address,
        DocumentVersion::new(version)?,
    )?))
}
fn event() -> Result<CommittedEvent, Box<dyn std::error::Error>> {
    let a = address(1)?;
    Ok(CommittedEvent {
        document_id: DocumentId::from_bytes([4; 16])?,
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: DocumentVersion::new(3)?,
        state_hash: [7; 32],
        operation: Operation::RangeWrite {
            writes: vec![
                GuardedContentWrite {
                    address: a,
                    content: Some(CellContent::Value(CellValue::Number(7.))),
                    previous: Some(formula(a, 1)?),
                },
                GuardedContentWrite {
                    address: address(2)?,
                    content: None,
                    previous: Some(CellContent::Value(CellValue::Boolean(true))),
                },
            ],
            kind: Some(RangeWriteKind::SetValues),
            undoes: None,
            undoes_version: None,
        },
    })
}
fn invalid(event: &CommittedEvent) {
    assert!(encode(event).is_err());
    assert!(event_hash(event).is_err());
}
#[test]
fn authored_and_compensating_range_roundtrip_and_distinct_hashes() -> TestResult {
    let authored = event()?;
    let bytes = encode(&authored)?;
    let dto = v1::EventEnvelope::decode(bytes.as_slice())?;
    assert_eq!(dto.schema_version, 17);
    assert_eq!(decode(&bytes)?, authored);
    let mut inverse = authored.clone();
    inverse.version = DocumentVersion::new(4)?;
    inverse.operation_id = OperationId::from_bytes([8; 16])?;
    if let Operation::RangeWrite {
        writes,
        kind,
        undoes,
        undoes_version,
    } = &mut inverse.operation
    {
        for write in writes {
            std::mem::swap(&mut write.content, &mut write.previous);
        }
        *kind = None;
        *undoes = Some(authored.operation_id);
        *undoes_version = Some(authored.version);
    }
    let bytes = encode(&inverse)?;
    assert_eq!(
        v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
        18
    );
    assert_eq!(decode(&bytes)?, inverse);
    assert_ne!(event_hash(&authored)?, event_hash(&inverse)?);
    let mut copy = authored.clone();
    if let Operation::RangeWrite { kind, writes, .. } = &mut copy.operation {
        *kind = Some(RangeWriteKind::Copy);
        writes[0].content = Some(formula(writes[0].address, 2)?);
    }
    assert_eq!(decode(&encode(&copy)?)?, copy);
    assert_ne!(event_hash(&copy)?, event_hash(&authored)?);
    Ok(())
}
#[test]
fn range_schema_and_wire_validation_are_strict() -> TestResult {
    let event = event()?;
    let dto = v1::EventEnvelope::decode(encode(&event)?.as_slice())?;
    for schema in [16, 18, 19] {
        let mut bad = dto.clone();
        bad.schema_version = schema;
        assert!(decode(&bad.encode_to_vec()).is_err());
    }
    for mutate in 0..5 {
        let mut bad = dto.clone();
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::AuthoredRangeWrite(range)),
        }) = &mut bad.operation
        {
            match mutate {
                0 => range.authored_kind = 99,
                1 => range.writes.push(range.writes[0].clone()),
                2 => range.writes.clear(),
                3 => range.undoes_version = 1,
                _ => range.writes[0].address = None,
            }
        }
        assert!(decode(&bad.encode_to_vec()).is_err());
    }
    Ok(())
}
#[test]
fn range_metadata_formulas_and_normalized_blanks_are_validated_in_encode_and_hash() -> TestResult {
    for mutation in 0..9 {
        let mut event = event()?;
        if let Operation::RangeWrite {
            writes,
            kind,
            undoes,
            undoes_version,
        } = &mut event.operation
        {
            match mutation {
                0 => writes.push(writes[0].clone()),
                1 => writes.clear(),
                2 => writes[0].content = Some(CellContent::Value(CellValue::Blank)),
                3 => writes[0].content = Some(formula(writes[0].address, 1)?),
                4 => writes[0].previous = Some(formula(writes[0].address, 3)?),
                5 => writes[0].previous = Some(formula(address(99)?, 1)?),
                6 => *kind = None,
                7 => {
                    *undoes = Some(OperationId::from_bytes([8; 16])?);
                    *undoes_version = Some(DocumentVersion::new(1)?);
                }
                _ => {
                    *kind = None;
                    *undoes = Some(OperationId::from_bytes([8; 16])?);
                    *undoes_version = Some(event.version);
                }
            }
        }
        invalid(&event);
    }
    let mut event = event()?;
    event.principal = EventPrincipal::System;
    invalid(&event);
    Ok(())
}
#[test]
fn range_incoming_and_preimage_text_have_independent_aggregate_caps() -> TestResult {
    for previous in [false, true] {
        let mut event = event()?;
        if let Operation::RangeWrite { writes, .. } = &mut event.operation {
            *writes = (0..17)
                .map(|index| {
                    Ok(GuardedContentWrite {
                        address: address(index)?,
                        content: if previous {
                            None
                        } else {
                            Some(CellContent::Value(CellValue::Text(
                                "x".repeat(MAX_TEXT_BYTES),
                            )))
                        },
                        previous: if previous {
                            Some(CellContent::Value(CellValue::Text(
                                "x".repeat(MAX_TEXT_BYTES),
                            )))
                        } else {
                            None
                        },
                    })
                })
                .collect::<Result<Vec<_>, DomainError>>()?;
        }
        invalid(&event);
    }
    let mut event = event()?;
    if let Operation::RangeWrite { writes, .. } = &mut event.operation {
        *writes = (0..1001)
            .map(|index| {
                Ok(GuardedContentWrite {
                    address: address(index)?,
                    content: None,
                    previous: None,
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
    }
    invalid(&event);
    Ok(())
}
#[test]
fn malformed_range_payloads_fail_decode_and_compensation_metadata_is_checked() -> TestResult {
    let authored = event()?;
    let dto = v1::EventEnvelope::decode(encode(&authored)?.as_slice())?;
    for mutation in 0..5 {
        let mut bad = dto.clone();
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::AuthoredRangeWrite(range)),
        }) = &mut bad.operation
        {
            match mutation {
                0 => {
                    range.writes[0].content = Some(v1::CellContent {
                        kind: Some(v1::cell_content::Kind::Value(v1::CellValue {
                            kind: Some(v1::cell_value::Kind::Blank(true)),
                        })),
                    })
                }
                1 => {
                    if let Some(v1::CellContent {
                        kind: Some(v1::cell_content::Kind::Formula(formula)),
                    }) = &mut range.writes[0].previous
                    {
                        formula.bound_at_version = 3;
                    }
                }
                2 => {
                    range.writes[0].content = range.writes[0].previous.clone();
                }
                3 => {
                    let mut write = range.writes[1].clone();
                    write.content = Some(v1::CellContent {
                        kind: Some(v1::cell_content::Kind::Value(v1::CellValue {
                            kind: Some(v1::cell_value::Kind::Text("x".repeat(MAX_TEXT_BYTES))),
                        })),
                    });
                    range.writes = (0u32..17)
                        .map(|index| {
                            let mut next = write.clone();
                            let mut row = [3; 16];
                            row[..4].copy_from_slice(&index.to_le_bytes());
                            if let Some(address) = &mut next.address {
                                address.row_id = row.to_vec();
                            }
                            next
                        })
                        .collect();
                }
                _ => {
                    if let Some(v1::CellContent {
                        kind: Some(v1::cell_content::Kind::Formula(formula)),
                    }) = &mut range.writes[0].previous
                        && let Some(origin) = &mut formula.origin
                    {
                        origin.row_id = vec![9; 16];
                    }
                }
            }
        }
        assert!(decode(&bad.encode_to_vec()).is_err());
    }
    let mut inverse = authored.clone();
    if let Operation::RangeWrite {
        kind,
        undoes,
        undoes_version,
        ..
    } = &mut inverse.operation
    {
        *kind = None;
        *undoes = Some(OperationId::from_bytes([8; 16])?);
        *undoes_version = Some(DocumentVersion::new(1)?);
    }
    let inverse_dto = v1::EventEnvelope::decode(encode(&inverse)?.as_slice())?;
    for mutation in 0..4 {
        let mut bad = inverse_dto.clone();
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::CompensatingRangeWrite(range)),
        }) = &mut bad.operation
        {
            match mutation {
                0 => range.authored_kind = 1,
                1 => range.undoes_version = 0,
                2 => range.undoes_version = 3,
                _ => range.undoes_operation_id.clear(),
            }
        }
        assert!(decode(&bad.encode_to_vec()).is_err());
    }
    if let Operation::RangeWrite { undoes_version, .. } = &mut inverse.operation {
        *undoes_version = Some(DocumentVersion::ZERO);
    }
    invalid(&inverse);
    Ok(())
}
#[test]
fn independent_text_caps_admit_the_exact_boundary() -> TestResult {
    let mut event = event()?;
    if let Operation::RangeWrite { writes, .. } = &mut event.operation {
        *writes = (0..16)
            .map(|index| {
                Ok(GuardedContentWrite {
                    address: address(index)?,
                    content: Some(CellContent::Value(CellValue::Text(
                        "x".repeat(MAX_TEXT_BYTES),
                    ))),
                    previous: Some(CellContent::Value(CellValue::Text(
                        "y".repeat(MAX_TEXT_BYTES),
                    ))),
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
    }
    assert_eq!(decode(&encode(&event)?)?, event);
    assert_eq!(event_hash(&decode(&encode(&event)?)?)?, event_hash(&event)?);
    Ok(())
}
