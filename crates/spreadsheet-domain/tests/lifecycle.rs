use spreadsheet_domain::{axis_edit::*, formula::*, lifecycle::*, workbook::*};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn fixture() -> Result<Workbook, DomainError> {
    Workbook::hydrate(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet".into(),
            rows: (3..8)
                .map(|n| RowId::from_bytes([n; 16]))
                .collect::<Result<_, _>>()?,
            columns: (8..13)
                .map(|n| ColumnId::from_bytes([n; 16]))
                .collect::<Result<_, _>>()?,
        },
        [],
    )
}
fn address(book: &Workbook, r: usize, c: usize) -> CellAddress {
    let s = &book.sheets()[0];
    CellAddress {
        sheet: s.id(),
        row: s.rows()[r],
        column: s.columns()[c],
    }
}
fn restored(base: &Workbook, columns: bool) -> Result<Workbook, Box<dyn std::error::Error>> {
    let s = &base.sheets()[0];
    let legacy = if columns {
        Operation::DeleteColumns {
            sheet: s.id(),
            columns: vec![s.columns()[2]],
        }
    } else {
        Operation::DeleteRows {
            sheet: s.id(),
            rows: vec![s.rows()[2]],
        }
    };
    let operation = prepare_axis_edit(base, &legacy, DocumentVersion::new(2)?)?;
    let after = base.candidate(&operation, DocumentVersion::new(2)?)?;
    let event = CommittedEvent {
        document_id: base.document_id(),
        operation_id: OperationId::from_bytes([20; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([19; 16])?),
        version: DocumentVersion::new(2)?,
        operation,
        state_hash: after.hash(),
    };
    let inverse = prepare_axis_inverse(&after, &event, DocumentVersion::new(3)?)?;
    Ok(after.candidate(&inverse, DocumentVersion::new(3)?)?)
}
fn command(book: &Workbook, operation: CommandPayload, base: u64) -> Result<Command, DomainError> {
    Ok(Command {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([21; 16])?,
        actor_id: ActorId::from_bytes([19; 16])?,
        base_version: DocumentVersion::new(base)?,
        client_sequence: 1,
        operation,
    })
}
#[test]
fn old_value_batch_and_axis_intents_cannot_cross_reactivation_but_independent_writes_can()
-> TestResult {
    for columns in [false, true] {
        let before = fixture()?;
        let book = restored(&before, columns)?;
        let revived = address(&before, 2, 2);
        let independent = address(&before, 0, 0);
        let payloads = vec![
            CommandPayload::SetCellValue {
                address: revived,
                value: CellValue::Number(1.),
            },
            CommandPayload::ClearCell { address: revived },
            CommandPayload::SetCellValues {
                writes: vec![
                    CellWrite {
                        address: independent,
                        value: CellValue::Number(2.),
                    },
                    CellWrite {
                        address: revived,
                        value: CellValue::Number(1.),
                    },
                ],
            },
            if columns {
                CommandPayload::InsertColumns {
                    sheet: revived.sheet,
                    before: revived.column,
                    columns: vec![ColumnId::from_bytes([30; 16])?],
                }
            } else {
                CommandPayload::InsertRows {
                    sheet: revived.sheet,
                    before: revived.row,
                    rows: vec![RowId::from_bytes([30; 16])?],
                }
            },
            if columns {
                CommandPayload::DeleteColumns {
                    sheet: revived.sheet,
                    columns: vec![revived.column],
                }
            } else {
                CommandPayload::DeleteRows {
                    sheet: revived.sheet,
                    rows: vec![revived.row],
                }
            },
        ];
        for payload in payloads {
            assert_eq!(
                check_command_activation(&book, &command(&book, payload.clone(), 1)?),
                Err(DomainError::VersionConflict)
            );
            check_command_activation(&book, &command(&book, payload, 3)?)?;
        }
        check_command_activation(
            &book,
            &command(
                &book,
                CommandPayload::SetCellValue {
                    address: independent,
                    value: CellValue::Number(7.),
                },
                1,
            )?,
        )?;
        assert_eq!(max_activation_version(&book), DocumentVersion::new(3)?);
        let image = book.snapshot_image()?;
        let hash = book.hash();
        let decoded = Workbook::restore_snapshot(image, hash)?;
        assert_eq!(
            check_command_activation(
                &decoded,
                &command(&decoded, CommandPayload::ClearCell { address: revived }, 1)?
            ),
            Err(DomainError::VersionConflict)
        );
    }
    Ok(())
}
#[test]
fn old_reversed_ranges_detect_reactivated_interiors_before_identity_is_lost() -> TestResult {
    for columns in [false, true] {
        let before = fixture()?;
        let head = restored(&before, columns)?;
        let make = |a: CellAddress| BoundCellReference {
            target_sheet: a.sheet,
            target_row: a.row,
            target_column: a.column,
            row_mode: AxisReferenceMode::Absolute,
            column_mode: AxisReferenceMode::Absolute,
        };
        let range = Node::Range(BoundRangeReference {
            start: make(address(&before, 4, 4)),
            end: make(address(&before, 0, 0)),
        });
        let formula = Formula::new(
            "=A5:A1".into(),
            "=A5:A1".into(),
            BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![range])?,
            address(&before, 0, 4),
            DocumentVersion::new(1)?,
        )?;
        assert_eq!(
            check_formula_activation(&head, &before, &formula, DocumentVersion::new(1)?),
            Err(DomainError::VersionConflict)
        );
        check_formula_activation(&head, &head, &formula, DocumentVersion::new(3)?)?;
        let scalar = Formula::new(
            "=A1".into(),
            "=A1".into(),
            BoundAst::new(
                AST_SCHEMA_VERSION,
                NodeId(0),
                vec![Node::Cell(make(address(&before, 0, 0)))],
            )?,
            address(&before, 0, 4),
            DocumentVersion::new(1)?,
        )?;
        check_formula_activation(&head, &before, &scalar, DocumentVersion::new(1)?)?;
    }
    Ok(())
}

#[test]
fn snapshot_cannot_lower_axis_metadata_below_hash_covered_activation() -> TestResult {
    let before = fixture()?;
    for columns in [false, true] {
        let head = restored(&before, columns)?;
        let mut image = head.snapshot_image()?;
        image.last_axis_change_version = DocumentVersion::new(1)?;
        assert!(matches!(
            Workbook::restore_snapshot(image, head.hash()),
            Err(DomainError::InvalidAxis)
        ));
    }
    Ok(())
}
