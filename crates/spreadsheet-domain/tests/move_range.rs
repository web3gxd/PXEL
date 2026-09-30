use spreadsheet_domain::workbook::*;

#[test]
fn omitted_inbound_formula_cannot_bypass_move_or_partial_range_guards()
-> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::formula::*;
    let (base, a) = fixture()?;
    let reference = |target: CellAddress, delta| BoundCellReference {
        target_sheet: target.sheet,
        target_row: target.row,
        target_column: target.column,
        row_mode: AxisReferenceMode::Absolute,
        column_mode: AxisReferenceMode::Relative {
            original_delta: delta,
        },
    };
    for node in [
        Node::Cell(reference(a[0], -3)),
        Node::Range(BoundRangeReference {
            start: reference(a[0], -3),
            end: reference(a[1], -2),
        }),
    ] {
        let formula = Formula::new(
            "=A1".into(),
            "=A1".into(),
            BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![node])?,
            a[3],
            base.version(),
        )?;
        let book = base.candidate(
            &Operation::SetCellFormula {
                address: a[3],
                formula,
            },
            DocumentVersion::new(2)?,
        )?;
        let hash = book.hash();
        let incomplete = Operation::MoveRange {
            source: a[0],
            destination: a[1],
            rows: 1,
            columns: 1,
            writes: vec![
                write(&book, a[0], None)?,
                write(&book, a[1], Some(CellValue::Number(1.0).into()))?,
            ],
        };
        assert!(matches!(
            book.candidate(&incomplete, DocumentVersion::new(3)?),
            Err(DomainError::InvalidFormula)
        ));
        assert_eq!(book.hash(), hash);
    }
    Ok(())
}

fn fixture() -> Result<(Workbook, Vec<CellAddress>), DomainError> {
    let sheet = SheetId::from_bytes([2; 16])?;
    let row = RowId::from_bytes([3; 16])?;
    let columns = (4..8)
        .map(|byte| ColumnId::from_bytes([byte; 16]))
        .collect::<Result<Vec<_>, _>>()?;
    let addresses: Vec<_> = columns
        .iter()
        .map(|column| CellAddress {
            sheet,
            row,
            column: *column,
        })
        .collect();
    let book = Workbook::hydrate(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows: vec![row],
            columns,
        },
        addresses
            .iter()
            .enumerate()
            .map(|(n, a)| (*a, CellValue::Number((n + 1) as f64).into())),
    )?;
    Ok((book, addresses))
}
fn write(
    book: &Workbook,
    address: CellAddress,
    value: Option<CellContent>,
) -> Result<GuardedContentWrite, DomainError> {
    Ok(GuardedContentWrite {
        address,
        content: value,
        previous: book.cell_content(address)?.cloned(),
    })
}
fn movement(book: &Workbook, a: &[CellAddress]) -> Result<Operation, DomainError> {
    Ok(Operation::MoveRange {
        source: a[0],
        destination: a[1],
        rows: 1,
        columns: 2,
        writes: vec![
            write(book, a[0], None)?,
            write(book, a[1], Some(CellValue::Number(1.0).into()))?,
            write(book, a[2], Some(CellValue::Number(2.0).into()))?,
        ],
    })
}

#[test]
fn overlapping_move_uses_frozen_source_and_one_version() -> Result<(), DomainError> {
    let (book, a) = fixture()?;
    let op = movement(&book, &a)?;
    let candidate = book.candidate(&op, DocumentVersion::new(2)?)?;
    assert_eq!(candidate.cell_content(a[0])?, None);
    assert_eq!(
        candidate.cell_content(a[1])?,
        Some(&CellValue::Number(1.0).into())
    );
    assert_eq!(
        candidate.cell_content(a[2])?,
        Some(&CellValue::Number(2.0).into())
    );
    assert_eq!(candidate.cell_content(a[3])?, book.cell_content(a[3])?);
    assert_eq!(candidate.version().get(), 2);
    let mut replay = book.clone();
    replay.apply_event(&CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([20; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([21; 16])?),
        version: candidate.version(),
        operation: op.clone(),
        state_hash: candidate.hash(),
    })?;
    assert_eq!(replay.hash(), candidate.hash());
    assert!(!op.is_inverse_target());
    assert!(!op.changes_axes());
    assert!(matches!(
        book.preview_candidate(&[op], book.version()),
        Err(DomainError::InvalidAxis)
    ));
    Ok(())
}

#[test]
fn malformed_move_footprints_and_guards_are_atomic() -> Result<(), DomainError> {
    let (book, a) = fixture()?;
    let hash = book.hash();
    for mutation in 0..6 {
        let mut op = movement(&book, &a)?;
        if let Operation::MoveRange { writes, .. } = &mut op {
            match mutation {
                0 => {
                    writes.pop();
                }
                1 => writes[0].previous = None,
                2 => writes.push(writes[0].clone()),
                3 => writes[1].content = Some(CellValue::Number(99.0).into()),
                4 => writes.push(write(&book, a[3], Some(CellValue::Number(99.0).into()))?),
                _ => writes[0].content = Some(CellValue::Number(1.0).into()),
            }
        }
        assert!(book.candidate(&op, DocumentVersion::new(2)?).is_err());
        assert_eq!(book.hash(), hash);
    }
    Ok(())
}

#[test]
fn geometry_limits_and_blank_footprint_are_checked() -> Result<(), DomainError> {
    let (book, a) = fixture()?;
    for (rows, columns, destination) in [
        (0, 1, a[1]),
        (1, 1001, a[1]),
        (2, 1, a[1]),
        (1, 2, a[3]),
        (1, 1, a[0]),
    ] {
        let op = Operation::MoveRange {
            source: a[0],
            destination,
            rows,
            columns,
            writes: vec![write(&book, a[0], None)?],
        };
        assert!(book.candidate(&op, DocumentVersion::new(2)?).is_err());
    }
    let book = book.candidate(
        &Operation::ClearCell { address: a[0] },
        DocumentVersion::new(2)?,
    )?;
    let mut op = Operation::MoveRange {
        source: a[0],
        destination: a[1],
        rows: 1,
        columns: 1,
        writes: vec![write(&book, a[0], None)?, write(&book, a[1], None)?],
    };
    let next = book.candidate(&op, DocumentVersion::new(3)?)?;
    assert!(next.cell_content(a[1])?.is_none());
    if let Operation::MoveRange { writes, .. } = &mut op {
        writes.remove(0);
    }
    assert!(matches!(
        book.candidate(&op, DocumentVersion::new(3)?),
        Err(DomainError::InvalidAxis)
    ));
    assert!(matches!(
        book.candidate(&op, book.version()),
        Err(DomainError::VersionConflict)
    ));
    Ok(())
}

#[test]
fn preimage_byte_limit_applies_even_when_all_new_contents_are_blank() -> Result<(), DomainError> {
    let (original, a) = fixture()?;
    let sheet = original.sheets().first().ok_or(DomainError::MissingSheet)?;
    let columns = (1_u128..=34)
        .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let addresses: Vec<_> = columns
        .iter()
        .map(|column| CellAddress {
            column: *column,
            ..a[0]
        })
        .collect();
    let book = Workbook::hydrate(
        original.document_id(),
        original.version(),
        SheetSeed {
            id: sheet.id(),
            name: sheet.name().into(),
            rows: sheet.rows().to_vec(),
            columns,
        },
        addresses[17..]
            .iter()
            .map(|a| (*a, CellValue::Text("x".repeat(MAX_TEXT_BYTES)).into())),
    )?;
    let hash = book.hash();
    let writes = addresses
        .iter()
        .map(|a| write(&book, *a, None))
        .collect::<Result<Vec<_>, _>>()?;
    let op = Operation::MoveRange {
        source: addresses[0],
        destination: addresses[17],
        rows: 1,
        columns: 17,
        writes,
    };
    assert!(matches!(
        book.candidate(&op, DocumentVersion::new(2)?),
        Err(DomainError::LimitExceeded)
    ));
    assert_eq!(book.hash(), hash);
    Ok(())
}

fn formula(at: CellAddress, n: f64) -> Result<CellContent, Box<dyn std::error::Error>> {
    use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};
    Ok(CellContent::Formula(Formula::new(
        format!("={n}"),
        format!("={n}"),
        BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(n)])?,
        at,
        DocumentVersion::new(1)?,
    )?))
}

#[test]
fn incoming_formula_bytes_and_write_count_have_independent_limits()
-> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};
    let (base, a) = fixture()?;
    let columns = (1_u128..=8)
        .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let at: Vec<_> = columns
        .iter()
        .map(|column| CellAddress {
            column: *column,
            ..a[0]
        })
        .collect();
    let mut cells = vec![(at[0], CellValue::Number(1.0).into())];
    for address in &at[2..] {
        cells.push((*address, formula(*address, 2.0)?));
    }
    let book = Workbook::hydrate(
        base.document_id(),
        base.version(),
        SheetSeed {
            id: a[0].sheet,
            name: "Sheet".into(),
            rows: vec![a[0].row],
            columns,
        },
        cells,
    )?;
    let mut writes = vec![
        write(&book, at[0], None)?,
        write(&book, at[1], Some(CellValue::Number(1.0).into()))?,
    ];
    for address in &at[2..] {
        let text = "x".repeat(32_760);
        let source = format!("=\"{text}\"");
        let content = CellContent::Formula(Formula::new(
            source.clone(),
            source,
            BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Text(text)])?,
            *address,
            book.version(),
        )?);
        writes.push(write(&book, *address, Some(content))?);
    }
    assert!(
        writes
            .iter()
            .filter_map(|w| w.previous.as_ref())
            .map(CellContent::text_bytes)
            .sum::<usize>()
            < 512 * 1024
    );
    let mut op = Operation::MoveRange {
        source: at[0],
        destination: at[1],
        rows: 1,
        columns: 1,
        writes,
    };
    assert!(matches!(
        book.candidate(&op, DocumentVersion::new(2)?),
        Err(DomainError::LimitExceeded)
    ));
    if let Operation::MoveRange { writes, .. } = &mut op {
        *writes = vec![write(&book, at[0], None)?; 10_001];
    }
    assert!(matches!(
        book.candidate(&op, DocumentVersion::new(2)?),
        Err(DomainError::LimitExceeded)
    ));
    Ok(())
}

#[test]
fn moved_and_external_formulas_require_origins_and_changed_ast()
-> Result<(), Box<dyn std::error::Error>> {
    let (base, a) = fixture()?;
    let sheet = &base.sheets()[0];
    let book = Workbook::hydrate(
        base.document_id(),
        base.version(),
        SheetSeed {
            id: sheet.id(),
            name: sheet.name().into(),
            rows: sheet.rows().to_vec(),
            columns: sheet.columns().to_vec(),
        },
        [
            (a[0], formula(a[0], 1.0)?),
            (a[3], reference_formula(a[3], a[0], -3)?),
        ],
    )?;
    let external_old = book.cell_content(a[3])?.ok_or("external formula missing")?;
    let CellContent::Formula(external_old) = external_old else {
        return Err("external content is not formula".into());
    };
    let CellContent::Formula(external_changed) = reference_formula(a[3], a[1], -2)? else {
        return Err("fixture is not formula".into());
    };
    let external_changed = CellContent::Formula(spreadsheet_domain::formula::Formula::new(
        external_old.source().to_owned(),
        external_old.canonical().to_owned(),
        external_changed.ast().clone(),
        a[3],
        book.version(),
    )?);
    let op = Operation::MoveRange {
        source: a[0],
        destination: a[1],
        rows: 1,
        columns: 1,
        writes: vec![
            write(&book, a[0], None)?,
            write(&book, a[1], Some(formula(a[1], 1.0)?))?,
            write(&book, a[3], Some(external_changed))?,
        ],
    };
    assert!(book.candidate(&op, DocumentVersion::new(2)?).is_ok());
    for forgery in 0..3 {
        use spreadsheet_domain::formula::*;
        let mut forged = op.clone();
        if let Operation::MoveRange { writes, .. } = &mut forged {
            let Some(CellContent::Formula(old)) = &writes[2].content else {
                return Err("formula missing".into());
            };
            let mut nodes = old.ast().nodes().to_vec();
            let Some(Node::Cell(reference)) = nodes.first_mut() else {
                return Err("reference missing".into());
            };
            match forgery {
                0 => reference.target_column = a[2].column,
                1 => reference.target_sheet = SheetId::from_bytes([90; 16])?,
                _ => reference.column_mode = AxisReferenceMode::Absolute,
            }
            writes[2].content = Some(CellContent::Formula(Formula::new(
                old.source().into(),
                old.canonical().into(),
                BoundAst::new(AST_SCHEMA_VERSION, old.ast().root(), nodes)?,
                a[3],
                book.version(),
            )?));
        }
        assert!(matches!(
            book.candidate(&forged, DocumentVersion::new(2)?),
            Err(DomainError::InvalidFormula)
        ));
    }
    let mut literal_tampered = op.clone();
    if let Operation::MoveRange { writes, .. } = &mut literal_tampered {
        let Some(CellContent::Formula(old)) = &writes[1].content else {
            return Err("formula missing".into());
        };
        use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};
        writes[1].content = Some(CellContent::Formula(Formula::new(
            old.source().into(),
            old.canonical().into(),
            BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(99.0)])?,
            a[1],
            book.version(),
        )?));
    }
    assert!(matches!(
        book.candidate(&literal_tampered, DocumentVersion::new(2)?),
        Err(DomainError::InvalidFormula)
    ));
    for index in [1, 2] {
        let mut tampered = op.clone();
        if let Operation::MoveRange { writes, .. } = &mut tampered {
            writes[index].content = Some(formula(writes[index].address, 99.0)?);
        }
        assert!(matches!(
            book.candidate(&tampered, DocumentVersion::new(2)?),
            Err(DomainError::InvalidFormula)
        ));
    }
    let mut unchanged = op.clone();
    if let Operation::MoveRange { writes, .. } = &mut unchanged {
        writes[2].content = writes[2].previous.clone();
    }
    assert!(matches!(
        book.candidate(&unchanged, DocumentVersion::new(2)?),
        Err(DomainError::InvalidFormula)
    ));
    let mut origin = op;
    if let Operation::MoveRange { writes, .. } = &mut origin {
        writes[1].content = Some(formula(a[0], 1.0)?);
    }
    assert!(book.candidate(&origin, DocumentVersion::new(2)?).is_err());
    Ok(())
}

fn reference_formula(
    at: CellAddress,
    target: CellAddress,
    delta: i64,
) -> Result<CellContent, Box<dyn std::error::Error>> {
    use spreadsheet_domain::formula::*;
    Ok(CellContent::Formula(Formula::new(
        "=A1".into(),
        "=A1".into(),
        BoundAst::new(
            AST_SCHEMA_VERSION,
            NodeId(0),
            vec![Node::Cell(BoundCellReference {
                target_sheet: target.sheet,
                target_row: target.row,
                target_column: target.column,
                row_mode: AxisReferenceMode::Absolute,
                column_mode: AxisReferenceMode::Relative {
                    original_delta: delta,
                },
            })],
        )?,
        at,
        DocumentVersion::new(1)?,
    )?))
}

#[test]
fn move_rejects_operator_forgery_with_unchanged_authored_text()
-> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::formula::*;
    let (base, a) = fixture()?;
    let build = |origin, operator| {
        Formula::new(
            "=1+2".into(),
            "=(1+2)".into(),
            BoundAst::new(
                AST_SCHEMA_VERSION,
                NodeId(2),
                vec![
                    Node::Number(1.0),
                    Node::Number(2.0),
                    Node::Binary {
                        op: operator,
                        left: NodeId(0),
                        right: NodeId(1),
                    },
                ],
            )?,
            origin,
            base.version(),
        )
    };
    let original = build(a[0], BinaryOp::Add)?;
    let book = base.candidate(
        &Operation::SetCellFormula {
            address: a[0],
            formula: original,
        },
        DocumentVersion::new(2)?,
    )?;
    let op = Operation::MoveRange {
        source: a[0],
        destination: a[1],
        rows: 1,
        columns: 1,
        writes: vec![
            write(&book, a[0], None)?,
            write(
                &book,
                a[1],
                Some(CellContent::Formula(build(a[1], BinaryOp::Subtract)?)),
            )?,
        ],
    };
    assert!(matches!(
        book.candidate(&op, DocumentVersion::new(3)?),
        Err(DomainError::InvalidFormula)
    ));
    Ok(())
}

#[test]
fn range_transform_rejects_partial_source_overlap() -> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::formula::*;
    let (base, a) = fixture()?;
    let reference = |target: CellAddress, delta| BoundCellReference {
        target_sheet: target.sheet,
        target_row: target.row,
        target_column: target.column,
        row_mode: AxisReferenceMode::Absolute,
        column_mode: AxisReferenceMode::Relative {
            original_delta: delta,
        },
    };
    for partial in [false, true] {
        let build = |range| {
            Formula::new(
                "=A1:B1".into(),
                "=A1:B1".into(),
                BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Range(range)])?,
                a[3],
                base.version(),
            )
        };
        let original = build(BoundRangeReference {
            start: reference(a[0], -3),
            end: if partial {
                reference(a[1], -2)
            } else {
                reference(a[0], -3)
            },
        })?;
        let book = base.candidate(
            &Operation::SetCellFormula {
                address: a[3],
                formula: original,
            },
            DocumentVersion::new(2)?,
        )?;
        let transformed = build(BoundRangeReference {
            start: reference(a[1], -2),
            end: reference(a[1], -2),
        })?;
        let op = Operation::MoveRange {
            source: a[0],
            destination: a[1],
            rows: 1,
            columns: 1,
            writes: vec![
                write(&book, a[0], None)?,
                write(&book, a[1], Some(CellValue::Number(1.0).into()))?,
                write(&book, a[3], Some(CellContent::Formula(transformed)))?,
            ],
        };
        let result = book.candidate(&op, DocumentVersion::new(3)?);
        if partial {
            assert!(matches!(result, Err(DomainError::InvalidFormula)));
        } else {
            assert!(result.is_ok());
        }
    }
    Ok(())
}
