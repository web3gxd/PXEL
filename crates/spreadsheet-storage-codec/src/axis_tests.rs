use super::*;
use spreadsheet_domain::style::{CellStyle, Change, StyleChange};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn fixture(compensating: bool) -> Result<CommittedEvent, Box<dyn std::error::Error>> {
    let row = RowId::from_bytes([3; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let change = AxisChange {
        sheet,
        delta: AxisDelta::Rows {
            removed: vec![row],
            inserted: vec![],
        },
        inverse_delta: AxisDelta::Rows {
            removed: vec![],
            inserted: vec![RowPlacement {
                row,
                before: Some(RowId::from_bytes([4; 16])?),
            }],
        },
        writes: vec![GuardedContentWrite {
            address: CellAddress {
                sheet,
                row,
                column: ColumnId::from_bytes([5; 16])?,
            },
            content: None,
            previous: Some(CellContent::Value(CellValue::Number(7.0))),
        }],
        style_writes: vec![],
        before_order_hash: [6; 32],
        after_order_hash: [7; 32],
    };
    Ok(CommittedEvent {
        document_id: DocumentId::from_bytes([1; 16])?,
        operation_id: OperationId::from_bytes([8; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([9; 16])?),
        version: DocumentVersion::new(4)?,
        operation: if compensating {
            Operation::AxisCompensation {
                change,
                undoes: OperationId::from_bytes([10; 16])?,
                undoes_version: DocumentVersion::new(3)?,
            }
        } else {
            Operation::AxisEdit { change }
        },
        state_hash: [11; 32],
    })
}
fn change(operation: &mut Operation) -> Result<&mut AxisChange, Box<dyn std::error::Error>> {
    match operation {
        Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => Ok(change),
        _ => Err("expected axis change".into()),
    }
}
#[test]
fn axis_event_schemas_roundtrip_and_bind_order_delta_preimage_and_inverse_identity() -> TestResult {
    for compensating in [false, true] {
        let event = fixture(compensating)?;
        let bytes = encode(&event).map_err(|error| format!("encode: {error}"))?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            if compensating { 22 } else { 21 }
        );
        assert_eq!(decode(&bytes)?, event);
        let original_hash = event_hash(&event)?;
        for mutation in 0..4 {
            let mut changed = event.clone();
            let change = change(&mut changed.operation)?;
            match mutation {
                0 => change.before_order_hash[0] ^= 1,
                1 => change.after_order_hash[0] ^= 1,
                2 => change.writes[0].previous = Some(CellContent::Value(CellValue::Number(8.0))),
                _ => {
                    if let AxisDelta::Rows { inserted, .. } = &mut change.inverse_delta {
                        inserted[0].before = None;
                    }
                }
            }
            assert_ne!(event_hash(&changed)?, original_hash);
        }
    }
    Ok(())
}

#[test]
fn styled_axis_events_use_additive_schemas_and_roundtrip_full_style_preimages() -> TestResult {
    for compensating in [false, true] {
        let mut event = fixture(compensating)?;
        let address = change(&mut event.operation)?.writes[0].address;
        let style = CellStyle::default()
            .apply(&[
                StyleChange::FontBold(Change::Set(true)),
                StyleChange::FontColor(Change::Set(spreadsheet_domain::style::Rgb::new([
                    12, 34, 56,
                ]))),
            ])
            .map_err(|error| format!("style apply: {error}"))?;
        change(&mut event.operation)?
            .style_writes
            .push(GuardedAxisStyleWrite {
                address,
                previous: Some(style.clone()),
                style: None,
            });
        let bytes = encode(&event).map_err(|error| format!("styled encode: {error}"))?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            if compensating { 27 } else { 26 }
        );
        assert_eq!(
            decode(&bytes).map_err(|error| format!("decode: {error}"))?,
            event
        );
        let hash = event_hash(&event).map_err(|error| format!("hash: {error}"))?;
        let mut changed = event.clone();
        if let Some(next) = change(&mut changed.operation)?.style_writes.first_mut() {
            next.previous =
                Some(CellStyle::default().apply(&[StyleChange::FontItalic(Change::Set(true))])?);
        }
        assert_ne!(event_hash(&changed)?, hash);
    }
    Ok(())
}
#[test]
fn axis_event_rejects_invalid_delta_inverse_order_duplicates_and_provenance() -> TestResult {
    for mutation in 0..9 {
        let mut event = fixture(true)?;
        let change = change(&mut event.operation)?;
        match mutation {
            0 => change.after_order_hash = change.before_order_hash,
            1 => {
                change.delta = AxisDelta::Rows {
                    removed: vec![],
                    inserted: vec![],
                }
            }
            2 => {
                if let AxisDelta::Rows { removed, .. } = &mut change.delta {
                    removed.push(removed[0]);
                }
            }
            3 => {
                change.inverse_delta = AxisDelta::Columns {
                    removed: vec![ColumnId::from_bytes([3; 16])?],
                    inserted: vec![],
                }
            }
            4 => change.writes.push(change.writes[0].clone()),
            5 => change.writes[0].content = Some(CellContent::Value(CellValue::Blank)),
            6 => {
                if let AxisDelta::Rows { inserted, .. } = &mut change.inverse_delta {
                    inserted[0].row = RowId::from_bytes([12; 16])?;
                }
            }
            7 => {
                if let Operation::AxisCompensation { undoes_version, .. } = &mut event.operation {
                    *undoes_version = DocumentVersion::ZERO;
                }
            }
            _ => event.principal = EventPrincipal::System,
        }
        assert!(encode(&event).is_err(), "mutation {mutation}");
        assert!(event_hash(&event).is_err(), "mutation {mutation}");
    }
    Ok(())
}

#[test]
fn axis_decode_rejects_wrong_schema_kind_missing_identity_and_malformed_change() -> TestResult {
    let bytes = encode(&fixture(true)?)?;
    for mutation in 0..19 {
        let mut envelope = v1::EventEnvelope::decode(bytes.as_slice())?;
        match mutation {
            0 => envelope.schema_version = 21,
            1 => envelope.schema_version = 23,
            2 => envelope.operation = Some(v1::Operation { kind: None }),
            3 => {
                envelope.principal_kind = 2;
                envelope.actor_id.clear();
            }
            _ => {
                let Some(v1::operation::Kind::AxisCompensation(comp)) =
                    envelope.operation.as_mut().and_then(|op| op.kind.as_mut())
                else {
                    return Err("wrong fixture kind".into());
                };
                match mutation {
                    4 => comp.undoes_operation_id.clear(),
                    5 => comp.undoes_operation_id = vec![0; 16],
                    6 => comp.undoes_version = 0,
                    7 => comp.undoes_version = envelope.committed_version,
                    8 => comp.change = None,
                    _ => {
                        let change = comp.change.as_mut().ok_or("missing change")?;
                        match mutation {
                            9 => change.sheet_id = vec![0; 16],
                            10 => change.delta = None,
                            11 => change.inverse_delta = None,
                            12 => change.delta = Some(v1::AxisDelta { kind: None }),
                            13 => change
                                .before_order_hash
                                .pop()
                                .map(|_| ())
                                .ok_or("missing hash")?,
                            14 => change.after_order_hash.clear(),
                            15 => change.writes[0].address = None,
                            16 => change.writes[0].previous = Some(v1::CellContent { kind: None }),
                            17 => {
                                change.writes[0].previous = Some(v1::CellContent {
                                    kind: Some(v1::cell_content::Kind::Value(v1::CellValue {
                                        kind: Some(v1::cell_value::Kind::Blank(true)),
                                    })),
                                })
                            }
                            _ => {
                                if let Some(v1::axis_delta::Kind::Rows(rows)) =
                                    change.delta.as_mut().and_then(|d| d.kind.as_mut())
                                {
                                    rows.removed[0] = vec![0; 16];
                                }
                            }
                        }
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
fn axis_footprint_text_order_and_formula_versions_are_bounded() -> TestResult {
    let mut event = fixture(false)?;
    let changed = change(&mut event.operation)?;
    changed.writes = (0..10_000)
        .map(|index| {
            Ok(GuardedContentWrite {
                address: super::move_tests::address(index)?,
                content: None,
                previous: None,
            })
        })
        .collect::<Result<_, DomainError>>()?;
    changed.writes.sort_by_key(|write| write.address);
    assert_eq!(decode(&encode(&event)?)?, event);
    let mut invalid = event.clone();
    change(&mut invalid.operation)?.writes.reverse();
    assert!(encode(&invalid).is_err());
    change(&mut event.operation)?
        .writes
        .push(GuardedContentWrite {
            address: super::move_tests::address(10_000)?,
            content: None,
            previous: None,
        });
    assert!(encode(&event).is_err());
    for preimage in [false, true] {
        let mut event = fixture(false)?;
        let changes = change(&mut event.operation)?;
        changes.writes = (0..16)
            .map(|index| {
                Ok(GuardedContentWrite {
                    address: super::move_tests::address(index)?,
                    content: (!preimage)
                        .then(|| CellContent::Value(CellValue::Text("a".repeat(32 * 1024)))),
                    previous: preimage
                        .then(|| CellContent::Value(CellValue::Text("a".repeat(32 * 1024)))),
                })
            })
            .collect::<Result<_, DomainError>>()?;
        changes.writes.sort_by_key(|write| write.address);
        assert_eq!(decode(&encode(&event)?)?, event);
        change(&mut event.operation)?
            .writes
            .push(GuardedContentWrite {
                address: super::move_tests::address(16)?,
                content: (!preimage).then(|| CellContent::Value(CellValue::Text("x".into()))),
                previous: preimage.then(|| CellContent::Value(CellValue::Text("x".into()))),
            });
        assert!(encode(&event).is_err());
        assert!(event_hash(&event).is_err());
    }
    let mut event = fixture(false)?;
    let write = &mut change(&mut event.operation)?.writes[0];
    write.content = Some(super::move_tests::formula(write.address, 4)?);
    write.previous = Some(super::move_tests::formula(write.address, 3)?);
    assert_eq!(decode(&encode(&event)?)?, event);
    change(&mut event.operation)?.writes[0].previous =
        change(&mut event.operation)?.writes[0].content.clone();
    assert!(encode(&event).is_err());
    Ok(())
}

#[test]
fn axis_row_and_column_batch_limits_allow_empty_cell_footprints() -> TestResult {
    for columns in [false, true] {
        let max = if columns { 100 } else { 1_000 };
        let mut event = fixture(false)?;
        for count in [max, max + 1] {
            let change = change(&mut event.operation)?;
            change.writes.clear();
            if columns {
                let ids = (0..count)
                    .map(|index| {
                        let mut bytes = [3; 16];
                        bytes[..4].copy_from_slice(&(index as u32).to_be_bytes());
                        ColumnId::from_bytes(bytes)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                change.delta = AxisDelta::Columns {
                    removed: ids.clone(),
                    inserted: vec![],
                };
                change.inverse_delta = AxisDelta::Columns {
                    removed: vec![],
                    inserted: ids
                        .into_iter()
                        .map(|column| ColumnPlacement {
                            column,
                            before: None,
                        })
                        .collect(),
                };
            } else {
                let ids = (0..count)
                    .map(|index| {
                        let mut bytes = [3; 16];
                        bytes[..4].copy_from_slice(&(index as u32).to_be_bytes());
                        RowId::from_bytes(bytes)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                change.delta = AxisDelta::Rows {
                    removed: ids.clone(),
                    inserted: vec![],
                };
                change.inverse_delta = AxisDelta::Rows {
                    removed: vec![],
                    inserted: ids
                        .into_iter()
                        .map(|row| RowPlacement { row, before: None })
                        .collect(),
                };
            }
            if count == max {
                assert_eq!(decode(&encode(&event)?)?, event);
            } else {
                assert!(encode(&event).is_err());
                assert!(event_hash(&event).is_err());
            }
        }
    }
    Ok(())
}

#[test]
fn axis_hash_golden_tags_23_and_24() -> TestResult {
    let hashes = [false, true]
        .into_iter()
        .map(|comp| {
            Ok(event_hash(&fixture(comp)?)?
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>())
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    // Cross-checked against an independent Python hashlib/struct implementation
    // of the explicit event/axis semantic encoding, not protobuf serialization.
    assert_eq!(
        hashes,
        [
            "d6aa11e47dcd27ba6aecdf31c1f264a88f05a2ef2fc117b374b165d5e56a5592",
            "819aa021d3f90bf1e7106fe59cdf6db8ee9ef006ee44f10942ab9b4beea1fd06",
        ]
    );
    Ok(())
}

#[test]
fn axis_events_keep_the_four_mib_envelope_cap_with_large_bound_asts() -> TestResult {
    use spreadsheet_domain::formula::{
        AST_SCHEMA_VERSION, BinaryOp, BoundAst, Formula, Node, NodeId,
    };
    let mut event = fixture(false)?;
    let axis = change(&mut event.operation)?;
    axis.writes = (0..10_000)
        .map(|index| {
            let address = super::move_tests::address(index)?;
            let mut nodes = vec![Node::Number(42.0)];
            for _ in 0..15 {
                let left = NodeId((nodes.len() - 1) as u32);
                nodes.push(Node::Number(42.0));
                let right = NodeId((nodes.len() - 1) as u32);
                nodes.push(Node::Binary {
                    op: BinaryOp::Add,
                    left,
                    right,
                });
            }
            let content = Some(CellContent::Formula(Formula::new(
                "=42".into(),
                "=42".into(),
                BoundAst::new(AST_SCHEMA_VERSION, NodeId((nodes.len() - 1) as u32), nodes)?,
                address,
                DocumentVersion::new(1)?,
            )?));
            Ok(GuardedContentWrite {
                address,
                content: content.clone(),
                previous: content,
            })
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    axis.writes.sort_by_key(|write| write.address);
    assert!(axis_codec::encode_change(axis)?.encoded_len() > 4 * 1024 * 1024);
    assert!(matches!(encode(&event), Err(CodecError::EventTooLarge)));
    let Operation::AxisEdit { change } = event.operation else {
        return Err("wrong fixture".into());
    };
    event.operation = Operation::AxisCompensation {
        change,
        undoes: OperationId::from_bytes([10; 16])?,
        undoes_version: DocumentVersion::new(3)?,
    };
    assert!(matches!(encode(&event), Err(CodecError::EventTooLarge)));
    Ok(())
}

fn commit_axis_fixture(
    book: &mut Workbook,
    operation: Operation,
) -> Result<CommittedEvent, Box<dyn std::error::Error>> {
    let next = DocumentVersion::new(book.version().get() + 1)?;
    let candidate = book.candidate(&operation, next)?;
    let mut id = [18; 16];
    id[..8].copy_from_slice(&next.get().to_le_bytes());
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes(id)?,
        principal: EventPrincipal::User(ActorId::from_bytes([19; 16])?),
        version: next,
        operation,
        state_hash: candidate.hash(),
    };
    let decoded = decode(&encode(&event)?)?;
    assert_eq!(decoded, event);
    book.apply_event(&decoded)?;
    Ok(event)
}
fn snapshot_restore(
    book: &Workbook,
    envelope: &v1::SnapshotEnvelope,
) -> Result<Workbook, CodecError> {
    decode_snapshot(
        &envelope.encode_to_vec(),
        book.document_id(),
        book.version(),
        book.last_axis_change_version(),
        book.hash(),
        "engine-v1",
        "pxel-excel-subset-v1",
    )
}
fn snapshot_axis_fixture() -> Result<(Workbook, CellAddress), Box<dyn std::error::Error>> {
    let document = DocumentId::from_bytes([1; 16])?;
    let address = CellAddress {
        sheet: SheetId::from_bytes([2; 16])?,
        row: RowId::from_bytes([3; 16])?,
        column: ColumnId::from_bytes([5; 16])?,
    };
    let mut book = Workbook::empty(document);
    commit_axis_fixture(
        &mut book,
        Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: address.sheet,
                name: "Axis fixture".into(),
                rows: vec![address.row, RowId::from_bytes([4; 16])?],
                columns: vec![address.column, ColumnId::from_bytes([6; 16])?],
            },
        },
    )?;
    commit_axis_fixture(
        &mut book,
        Operation::SetCellValue {
            address,
            value: CellValue::Number(7.0),
        },
    )?;
    Ok((book, address))
}
#[test]
fn snapshot_v5_roundtrips_sparse_row_and_column_sizes() -> TestResult {
    let (mut book, address) = snapshot_axis_fixture()?;
    let row = AxisSizeTarget::Row(address.row);
    let column = AxisSizeTarget::Column(address.column);
    let event = commit_axis_fixture(
        &mut book,
        Operation::AxisSizeWrite {
            sheet: address.sheet,
            writes: vec![
                AxisSizeChange {
                    target: row,
                    previous: None,
                    size: Some(44),
                },
                AxisSizeChange {
                    target: column,
                    previous: None,
                    size: Some(176),
                },
            ],
        },
    )?;
    assert_eq!(decode(&encode(&event)?)?, event);
    let envelope = snapshot_envelope(&book)?;
    let encoded = envelope.sheets.first().ok_or("missing snapshot sheet")?;
    assert_eq!(encoded.axis_sizes.len(), 2);
    assert_eq!(snapshot_restore(&book, &envelope)?.hash(), book.hash());
    let mut downgraded = envelope.clone();
    downgraded.snapshot_schema_version = 4;
    assert!(matches!(
        decode_snapshot(
            &downgraded.encode_to_vec(),
            book.document_id(),
            book.version(),
            book.last_axis_change_version(),
            book.hash(),
            "engine-v1",
            "pxel-excel-subset-v1"
        ),
        Err(CodecError::Invalid)
    ));
    Ok(())
}
fn snapshot_envelope(book: &Workbook) -> Result<v1::SnapshotEnvelope, Box<dyn std::error::Error>> {
    let bytes = encode_snapshot(book, "engine-v1", "pxel-excel-subset-v1")?;
    let envelope = v1::SnapshotEnvelope::decode(bytes.as_slice())?;
    assert_eq!(
        envelope.snapshot_schema_version,
        crate::SNAPSHOT_SCHEMA_VERSION
    );
    assert_eq!(snapshot_restore(book, &envelope)?.hash(), book.hash());
    Ok(envelope)
}
#[test]
fn snapshot_v3_preserves_live_revived_and_redeleted_axis_floors_through_tail_replay() -> TestResult
{
    use spreadsheet_domain::axis_edit::{prepare_axis_edit, prepare_axis_inverse};
    let (mut book, address) = snapshot_axis_fixture()?;
    let baseline = snapshot_envelope(&book)?;
    for legacy in [1, 2] {
        let mut old = baseline.clone();
        old.snapshot_schema_version = legacy;
        for sheet in &mut old.sheets {
            sheet.settings = None;
        }
        if legacy == 1 {
            let single = old.sheets.remove(0);
            old.sheet = single.sheet;
            old.cells = single.cells;
        }
        assert_eq!(snapshot_restore(&book, &old)?.hash(), book.hash());
    }
    for columns in [false, true] {
        let deletion = if columns {
            Operation::DeleteColumns {
                sheet: address.sheet,
                columns: vec![address.column],
            }
        } else {
            Operation::DeleteRows {
                sheet: address.sheet,
                rows: vec![address.row],
            }
        };
        let original = prepare_axis_edit(
            &book,
            &deletion,
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        let mut target = commit_axis_fixture(&mut book, original)?;
        snapshot_envelope(&book)?;
        for active in [true, false, true] {
            let inverse = prepare_axis_inverse(
                &book,
                &target,
                DocumentVersion::new(book.version().get() + 1)?,
            )?;
            target = commit_axis_fixture(&mut book, inverse)?;
            let envelope = snapshot_envelope(&book)?;
            let floor = if columns {
                envelope
                    .column_activation_floors
                    .first()
                    .map(|f| f.document_version)
            } else {
                envelope
                    .row_activation_floors
                    .first()
                    .map(|f| f.document_version)
            }
            .ok_or("missing activation floor")?;
            assert_eq!(floor, book.version().get() - u64::from(!active));
            let image = book.snapshot_image()?;
            if columns {
                assert!(image.retired_columns.contains(&address.column));
            } else {
                assert!(image.retired_rows.contains(&address.row));
            }
            if active {
                assert_eq!(
                    book.cell_content(address)?,
                    Some(&CellContent::Value(CellValue::Number(7.0)))
                );
            } else if columns {
                assert!(matches!(
                    book.cell_content(address),
                    Err(DomainError::MissingColumn)
                ));
            } else {
                assert!(matches!(
                    book.cell_content(address),
                    Err(DomainError::MissingRow)
                ));
            }
            // Each subsequent inverse starts from decoded snapshot state, proving
            // that retained lifetime IDs and activation floors survive recovery.
            book = snapshot_restore(&book, &envelope)?;
        }
    }
    Ok(())
}
#[test]
fn snapshot_v3_refuses_missing_duplicate_foreign_and_downgraded_activation_metadata() -> TestResult
{
    use spreadsheet_domain::axis_edit::{prepare_axis_edit, prepare_axis_inverse};
    let (mut book, address) = snapshot_axis_fixture()?;
    let original = prepare_axis_edit(
        &book,
        &Operation::DeleteRows {
            sheet: address.sheet,
            rows: vec![address.row],
        },
        DocumentVersion::new(3)?,
    )?;
    let event = commit_axis_fixture(&mut book, original)?;
    let inverse = prepare_axis_inverse(&book, &event, DocumentVersion::new(4)?)?;
    commit_axis_fixture(&mut book, inverse)?;
    let envelope = snapshot_envelope(&book)?;
    for mutation in 0..10 {
        let mut invalid = envelope.clone();
        match mutation {
            0 => invalid.row_activation_floors.clear(),
            1 => invalid
                .row_activation_floors
                .push(invalid.row_activation_floors[0].clone()),
            2 => invalid.row_activation_floors[0].document_version = 0,
            3 => invalid.row_activation_floors[0].document_version = book.version().get() + 1,
            4 => invalid.row_activation_floors[0].row_id = vec![0; 16],
            5 => invalid.row_activation_floors[0].row_id = vec![12; 16],
            6 => invalid.retired_rows.clear(),
            7 => invalid.snapshot_schema_version = 2,
            8 => invalid.snapshot_schema_version = 1,
            _ => invalid
                .column_activation_floors
                .push(v1::ColumnActivationFloor {
                    column_id: address.column.as_bytes().to_vec(),
                    document_version: book.version().get(),
                }),
        }
        assert!(
            snapshot_restore(&book, &invalid).is_err(),
            "mutation {mutation}"
        );
    }
    Ok(())
}
