use super::move_tests::{address, formula};
use super::*;
use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BinaryOp, BoundAst, Formula, Node, NodeId};
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture() -> Result<CommittedEvent, Box<dyn std::error::Error>> {
    let mut event = super::move_tests::fixture()?;
    let Operation::MoveRange {
        source,
        destination,
        rows,
        columns,
        writes,
    } = event.operation
    else {
        return Err("fixture is not a move".into());
    };
    event.operation = Operation::MoveCompensation {
        source,
        destination,
        rows,
        columns,
        writes: writes
            .into_iter()
            .map(|w| GuardedContentWrite {
                address: w.address,
                content: w.previous,
                previous: w.content,
            })
            .collect(),
        undoes: OperationId::from_bytes([8; 16])?,
        undoes_version: DocumentVersion::new(2)?,
    };
    Ok(event)
}
fn invalid(event: &CommittedEvent) {
    assert!(encode(event).is_err());
    assert!(event_hash(event).is_err());
}
#[test]
fn move_compensation_roundtrip_hash_binds_target_geometry_and_ordered_preimages() -> TestResult {
    let event = fixture()?;
    let bytes = encode(&event)?;
    assert_eq!(
        v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
        20
    );
    assert_eq!(decode(&bytes)?, event);
    assert_eq!(event_hash(&decode(&bytes)?)?, event_hash(&event)?);
    for mutation in 0..6 {
        let mut changed = event.clone();
        if let Operation::MoveCompensation {
            destination,
            rows,
            writes,
            undoes,
            undoes_version,
            ..
        } = &mut changed.operation
        {
            match mutation {
                0 => *destination = address(3)?,
                1 => *rows = 2,
                2 => writes.reverse(),
                3 => writes[0].previous = Some(CellContent::Value(CellValue::Boolean(true))),
                4 => *undoes = OperationId::from_bytes([9; 16])?,
                _ => *undoes_version = DocumentVersion::new(1)?,
            }
        }
        assert_ne!(event_hash(&changed)?, event_hash(&event)?);
    }
    let mut original = event.clone();
    if let Operation::MoveCompensation {
        source,
        destination,
        rows,
        columns,
        writes,
        ..
    } = original.operation
    {
        original.operation = Operation::MoveRange {
            source,
            destination,
            rows,
            columns,
            writes,
        };
    }
    assert_ne!(event_hash(&original)?, event_hash(&event)?);
    Ok(())
}
#[test]
fn move_compensation_encode_and_hash_reject_invalid_geometry_metadata_and_formulas() -> TestResult {
    for mutation in 0..12 {
        let mut event = fixture()?;
        if let Operation::MoveCompensation {
            source,
            destination,
            rows,
            columns,
            writes,
            undoes_version,
            ..
        } = &mut event.operation
        {
            match mutation {
                0 => *destination = *source,
                1 => destination.sheet = SheetId::from_bytes([9; 16])?,
                2 => *rows = 0,
                3 => *columns = 1001,
                4 => writes.clear(),
                5 => writes.push(writes[0].clone()),
                6 => writes[0].content = Some(CellContent::Value(CellValue::Blank)),
                7 => writes[0].content = Some(formula(*destination, 1)?),
                8 => writes[0].content = Some(formula(writes[0].address, 3)?),
                9 => writes[1].previous = Some(formula(writes[1].address, 3)?),
                10 => *undoes_version = DocumentVersion::ZERO,
                _ => *undoes_version = DocumentVersion::new(3)?,
            }
        }
        invalid(&event);
    }
    let mut event = fixture()?;
    event.principal = EventPrincipal::System;
    invalid(&event);
    event.principal = EventPrincipal::User(ActorId::from_bytes([6; 16])?);
    event.version = DocumentVersion::ZERO;
    invalid(&event);
    Ok(())
}
#[test]
fn move_compensation_decode_rejects_downgrade_unknown_and_malformed_metadata() -> TestResult {
    let bytes = encode(&fixture()?)?;
    for mutation in 0..15 {
        let mut envelope = v1::EventEnvelope::decode(bytes.as_slice())?;
        match mutation {
            0 => envelope.schema_version = 19,
            1 => envelope.schema_version = 21,
            2 => {
                envelope.principal_kind = 2;
                envelope.actor_id.clear();
            }
            _ => {
                let Some(v1::operation::Kind::MoveCompensation(comp)) =
                    envelope.operation.as_mut().and_then(|o| o.kind.as_mut())
                else {
                    return Err("wrong encoded kind".into());
                };
                match mutation {
                    3 => comp.undoes_operation_id.clear(),
                    4 => comp.undoes_operation_id = vec![0; 16],
                    5 => comp.undoes_version = 0,
                    6 => comp.undoes_version = envelope.committed_version,
                    7 => comp.source = None,
                    8 => comp.destination = comp.source.clone(),
                    9 => comp.writes.push(comp.writes[0].clone()),
                    10 => comp.rows = 0,
                    11 => comp.writes[0].address = None,
                    12 => {
                        comp.writes[0].content = encode_optional_content(&Some(formula(
                            address(1)?,
                            envelope.committed_version,
                        )?))?
                    }
                    13 => {
                        comp.writes[0].previous = encode_optional_content(&Some(formula(
                            address(1)?,
                            envelope.committed_version,
                        )?))?
                    }
                    _ => {
                        comp.writes[0].content = Some(v1::CellContent {
                            kind: Some(v1::cell_content::Kind::Value(v1::CellValue {
                                kind: Some(v1::cell_value::Kind::Blank(true)),
                            })),
                        })
                    }
                }
            }
        }
        assert!(
            decode(&envelope.encode_to_vec()).is_err(),
            "mutation {mutation}"
        );
    }
    Ok(())
}
#[test]
fn move_compensation_footprint_and_independent_text_caps_are_enforced() -> TestResult {
    let mut event = fixture()?;
    if let Operation::MoveCompensation { writes, .. } = &mut event.operation {
        *writes = (0..10_000)
            .map(|n| {
                Ok(GuardedContentWrite {
                    address: address(n)?,
                    content: None,
                    previous: None,
                })
            })
            .collect::<Result<_, DomainError>>()?;
    }
    assert_eq!(decode(&encode(&event)?)?, event);
    if let Operation::MoveCompensation { writes, .. } = &mut event.operation {
        writes.push(GuardedContentWrite {
            address: address(10_000)?,
            content: None,
            previous: None,
        });
    }
    invalid(&event);
    for preimage in [false, true] {
        let mut event = fixture()?;
        if let Operation::MoveCompensation { writes, .. } = &mut event.operation {
            *writes = (0..16)
                .map(|n| {
                    let value = Some(CellContent::Value(CellValue::Text("x".repeat(32 * 1024))));
                    Ok(GuardedContentWrite {
                        address: address(n)?,
                        content: value.clone(),
                        previous: value,
                    })
                })
                .collect::<Result<_, DomainError>>()?;
        }
        assert_eq!(decode(&encode(&event)?)?, event);
        if let Operation::MoveCompensation { writes, .. } = &mut event.operation {
            writes.push(GuardedContentWrite {
                address: address(16)?,
                content: (!preimage).then(|| CellContent::Value(CellValue::Text("x".into()))),
                previous: preimage.then(|| CellContent::Value(CellValue::Text("x".into()))),
            });
        }
        invalid(&event);
    }
    Ok(())
}

#[test]
fn move_compensation_retains_four_mib_encoded_envelope_cap() -> TestResult {
    let mut event = fixture()?;
    if let Operation::MoveCompensation { writes, .. } = &mut event.operation {
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
                let content = Some(CellContent::Formula(Formula::new(
                    "=42".into(),
                    "=42".into(),
                    BoundAst::new(AST_SCHEMA_VERSION, root, nodes)?,
                    address,
                    DocumentVersion::new(1)?,
                )?));
                Ok(GuardedContentWrite {
                    address,
                    content: content.clone(),
                    previous: content,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    }
    let compensation = move_compensation_codec::encode(&event.operation)?;
    assert!(compensation.encoded_len() > 4 * 1024 * 1024);
    assert!(matches!(encode(&event), Err(CodecError::EventTooLarge)));
    assert!(decode(&compensation.encode_to_vec()).is_err());
    Ok(())
}
