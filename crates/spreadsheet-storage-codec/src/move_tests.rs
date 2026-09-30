use super::*;
use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BinaryOp, BoundAst, Formula, Node, NodeId};

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
fn fixture() -> Result<CommittedEvent, Box<dyn std::error::Error>> {
    let source = address(1)?;
    let destination = address(2)?;
    Ok(CommittedEvent {
        document_id: DocumentId::from_bytes([4; 16])?,
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: DocumentVersion::new(3)?,
        state_hash: [7; 32],
        operation: Operation::MoveRange {
            source,
            destination,
            rows: 1,
            columns: 1,
            writes: vec![
                GuardedContentWrite {
                    address: source,
                    content: None,
                    previous: Some(formula(source, 1)?),
                },
                GuardedContentWrite {
                    address: destination,
                    content: Some(formula(destination, 2)?),
                    previous: Some(CellContent::Value(CellValue::Number(7.))),
                },
            ],
        },
    })
}
fn invalid(event: &CommittedEvent) {
    assert!(encode(event).is_err());
    assert!(event_hash(event).is_err());
}
#[test]
fn frozen_move_roundtrip_binds_geometry_ordered_footprint_and_preimages() -> TestResult {
    let event = fixture()?;
    let bytes = encode(&event)?;
    assert_eq!(
        v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
        19
    );
    assert_eq!(decode(&bytes)?, event);
    assert_eq!(event_hash(&decode(&bytes)?)?, event_hash(&event)?);
    for mutation in 0..4 {
        let mut changed = event.clone();
        if let Operation::MoveRange {
            destination,
            rows,
            writes,
            ..
        } = &mut changed.operation
        {
            match mutation {
                0 => *destination = address(3)?,
                1 => *rows = 2,
                2 => writes.reverse(),
                _ => writes[1].previous = None,
            }
        }
        assert_ne!(event_hash(&changed)?, event_hash(&event)?);
    }
    Ok(())
}
#[test]
fn frozen_move_encoder_and_hash_reject_geometry_metadata_and_formula_boundaries() -> TestResult {
    for mutation in 0..9 {
        let mut event = fixture()?;
        if let Operation::MoveRange {
            source,
            destination,
            rows,
            columns,
            writes,
        } = &mut event.operation
        {
            match mutation {
                0 => *destination = *source,
                1 => destination.sheet = SheetId::from_bytes([9; 16])?,
                2 => *rows = 0,
                3 => {
                    *rows = u32::MAX;
                    *columns = u32::MAX;
                }
                4 => writes.clear(),
                5 => writes.push(writes[0].clone()),
                6 => writes[0].content = Some(CellContent::Value(CellValue::Blank)),
                7 => writes[0].previous = Some(formula(writes[0].address, 3)?),
                _ => writes[1].content = Some(formula(address(99)?, 1)?),
            }
        }
        invalid(&event);
    }
    let mut event = fixture()?;
    event.principal = EventPrincipal::System;
    invalid(&event);
    Ok(())
}
#[test]
fn frozen_move_decoder_rejects_downgrades_unknowns_and_malformed_payloads() -> TestResult {
    let dto = v1::EventEnvelope::decode(encode(&fixture()?)?.as_slice())?;
    for schema in [18, 20] {
        let mut bad = dto.clone();
        bad.schema_version = schema;
        assert!(decode(&bad.encode_to_vec()).is_err());
    }
    for mutation in 0..7 {
        let mut bad = dto.clone();
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::MoveRange(moved)),
        }) = &mut bad.operation
        {
            match mutation {
                0 => moved.source = None,
                1 => moved.destination = moved.source.clone(),
                2 => moved.columns = 0,
                3 => moved.writes.push(moved.writes[0].clone()),
                4 => moved.writes[0].address = None,
                5 => {
                    if let Some(v1::CellContent {
                        kind: Some(v1::cell_content::Kind::Formula(formula)),
                    }) = &mut moved.writes[1].content
                    {
                        formula.bound_at_version = 3;
                    }
                }
                _ => {
                    if let Some(v1::CellContent {
                        kind: Some(v1::cell_content::Kind::Formula(formula)),
                    }) = &mut moved.writes[0].previous
                    {
                        formula.compatibility_profile = 99;
                    }
                }
            }
        }
        assert!(decode(&bad.encode_to_vec()).is_err());
    }
    Ok(())
}
#[test]
fn frozen_move_write_count_and_independent_text_caps_are_enforced() -> TestResult {
    let mut event = fixture()?;
    if let Operation::MoveRange { writes, .. } = &mut event.operation {
        *writes = (0..10_000)
            .map(|index| {
                Ok(GuardedContentWrite {
                    address: address(index)?,
                    content: None,
                    previous: None,
                })
            })
            .collect::<Result<_, DomainError>>()?;
    }
    assert_eq!(decode(&encode(&event)?)?, event);
    if let Operation::MoveRange { writes, .. } = &mut event.operation {
        writes.push(GuardedContentWrite {
            address: address(10_000)?,
            content: None,
            previous: None,
        });
    }
    invalid(&event);
    for previous in [false, true] {
        let mut event = fixture()?;
        if let Operation::MoveRange { writes, .. } = &mut event.operation {
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
                .collect::<Result<_, DomainError>>()?;
        }
        invalid(&event);
    }
    Ok(())
}
#[test]
fn move_command_fingerprint_is_distinct_and_binds_each_geometry_field() -> TestResult {
    let event = fixture()?;
    let Operation::MoveRange {
        source,
        destination,
        rows,
        columns,
        ..
    } = &event.operation
    else {
        return Err("unexpected fixture".into());
    };
    let command = Command {
        document_id: event.document_id,
        operation_id: event.operation_id,
        actor_id: ActorId::from_bytes([6; 16])?,
        base_version: DocumentVersion::new(2)?,
        client_sequence: 1,
        operation: CommandPayload::MoveRange {
            source: *source,
            destination: *destination,
            rows: *rows,
            columns: *columns,
        },
    };
    let original = command_fingerprint(&command)?;
    let mut copy = command.clone();
    copy.operation = CommandPayload::CopyRange {
        source: *source,
        destination: *destination,
        rows: *rows,
        columns: *columns,
    };
    assert_ne!(command_fingerprint(&copy)?, original);
    for mutation in 0..4 {
        let mut changed = command.clone();
        if let CommandPayload::MoveRange {
            source,
            destination,
            rows,
            columns,
        } = &mut changed.operation
        {
            match mutation {
                0 => *source = address(4)?,
                1 => *destination = address(5)?,
                2 => *rows = 2,
                _ => *columns = 2,
            }
        }
        assert_ne!(command_fingerprint(&changed)?, original);
    }
    Ok(())
}
#[test]
fn frozen_move_retains_the_four_mib_encoded_envelope_cap() -> TestResult {
    let mut event = fixture()?;
    if let Operation::MoveRange { writes, .. } = &mut event.operation {
        *writes = (0..10_000)
            .map(|index| {
                let address = address(index)?;
                let mut nodes = vec![Node::Number(42.)];
                for _ in 0..15 {
                    let left = NodeId((nodes.len() - 1) as u32);
                    nodes.push(Node::Number(42.));
                    let right = NodeId((nodes.len() - 1) as u32);
                    nodes.push(Node::Binary {
                        op: BinaryOp::Add,
                        left,
                        right,
                    });
                }
                let root = NodeId((nodes.len() - 1) as u32);
                let formula = Formula::new(
                    "=42".into(),
                    "=42".into(),
                    BoundAst::new(AST_SCHEMA_VERSION, root, nodes)?,
                    address,
                    DocumentVersion::new(1)?,
                )?;
                let content = Some(CellContent::Formula(formula));
                Ok(GuardedContentWrite {
                    address,
                    content: content.clone(),
                    previous: content,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    }
    let moved = move_codec::encode(&event.operation)?;
    assert!(moved.encoded_len() > 4 * 1024 * 1024);
    assert!(matches!(encode(&event), Err(CodecError::EventTooLarge)));
    Ok(())
}
