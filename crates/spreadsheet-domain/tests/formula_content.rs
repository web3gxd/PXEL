use spreadsheet_domain::{formula::*, workbook::*};

fn fixture() -> Result<(Workbook, CellAddress), Box<dyn std::error::Error>> {
    let document = DocumentId::from_bytes([1; 16])?;
    let address = CellAddress {
        sheet: SheetId::from_bytes([2; 16])?,
        row: RowId::from_bytes([3; 16])?,
        column: ColumnId::from_bytes([4; 16])?,
    };
    let book = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: address.sheet,
                name: "Sheet".into(),
                rows: vec![address.row],
                columns: vec![address.column],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    Ok((book, address))
}
fn literal(
    address: CellAddress,
    source: &str,
    value: f64,
) -> Result<Formula, Box<dyn std::error::Error>> {
    Ok(Formula::new(
        source.into(),
        source.into(),
        BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(value)])?,
        address,
        DocumentVersion::new(1)?,
    )?)
}

#[test]
fn formula_is_authored_content_and_replays_without_calculated_cache()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut book, address) = fixture()?;
    let formula = literal(address, "=42", 42.0)?;
    let op = Operation::SetCellFormula {
        address,
        formula: formula.clone(),
    };
    let next = book.candidate(&op, DocumentVersion::new(2)?)?;
    assert!(
        matches!(next.sheets()[0].cells().get(&address),Some(CellContent::Formula(stored)) if stored==&formula)
    );
    assert_eq!(next.text_bytes(), formula.text_bytes());
    assert_eq!(next.ast_memory_bytes(), formula.ast_memory_bytes());
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: next.version(),
        operation: op,
        state_hash: next.hash(),
    };
    book.apply_event(&event)?;
    assert_eq!(book.hash(), next.hash());
    let value = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(42.0),
        },
        DocumentVersion::new(3)?,
    )?;
    assert_eq!(value.ast_memory_bytes(), 0);
    assert_eq!(value.text_bytes(), 0);
    assert_ne!(value.hash(), book.hash());
    Ok(())
}

#[test]
fn formula_source_intent_is_bounded_redacted_and_separate_from_bound_operation()
-> Result<(), Box<dyn std::error::Error>> {
    assert!(FormulaSource::new("=".into()).is_err());
    assert!(FormulaSource::new("text".into()).is_err());
    assert!(FormulaSource::new(format!("={}", "x".repeat(32768))).is_err());
    let source = FormulaSource::new("=privatevalue()".into())?;
    assert_eq!(source.as_str(), "=privatevalue()");
    assert!(!format!("{source:?}").contains("privatevalue"));
    let (_, address) = fixture()?;
    let _intent = CommandPayload::SetCellFormula { address, source };
    Ok(())
}

#[test]
fn formula_hash_has_independent_golden_and_covers_authored_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let (_, address) = fixture()?;
    let formula = literal(address, "=42", 42.0)?;
    assert_eq!(
        formula.semantic_hash().map(|b| format!("{b:02x}")).concat(),
        "3aebf86db7ccb2e25713798eaeadaab1acf714a031efa1317b378b782a28c43f"
    );
    let mut hashes = std::collections::HashSet::new();
    for value in [
        formula.clone(),
        literal(address, "= 42", 42.0)?,
        literal(address, "=42", 43.0)?,
        Formula::new(
            "=42".into(),
            "=(42)".into(),
            formula.ast().clone(),
            address,
            DocumentVersion::new(1)?,
        )?,
        Formula::new(
            "=42".into(),
            "=42".into(),
            formula.ast().clone(),
            address,
            DocumentVersion::new(2)?,
        )?,
        literal(
            CellAddress {
                row: RowId::from_bytes([9; 16])?,
                ..address
            },
            "=42",
            42.0,
        )?,
    ] {
        assert!(hashes.insert(value.semantic_hash()));
    }
    assert!(std::mem::size_of::<Node>() <= 256);
    Ok(())
}

#[test]
fn invalid_origin_future_base_and_dead_bound_targets_are_atomic()
-> Result<(), Box<dyn std::error::Error>> {
    let (book, address) = fixture()?;
    let hash = book.hash();
    let other = CellAddress {
        row: RowId::from_bytes([9; 16])?,
        ..address
    };
    let bad_origin = literal(other, "=1", 1.0)?;
    let future = Formula::new(
        "=1".into(),
        "=1".into(),
        BoundAst::new(1, NodeId(0), vec![Node::Number(1.0)])?,
        address,
        DocumentVersion::new(3)?,
    )?;
    for formula in [bad_origin, future] {
        assert!(matches!(
            book.candidate(
                &Operation::SetCellFormula { address, formula },
                DocumentVersion::new(2)?
            ),
            Err(DomainError::InvalidFormula)
        ));
    }
    let missing = BoundCellReference {
        target_sheet: address.sheet,
        target_row: other.row,
        target_column: address.column,
        row_mode: AxisReferenceMode::Absolute,
        column_mode: AxisReferenceMode::Absolute,
    };
    let formula = Formula::new(
        "=A2".into(),
        "=A2".into(),
        BoundAst::new(1, NodeId(0), vec![Node::Cell(missing)])?,
        address,
        DocumentVersion::new(1)?,
    )?;
    assert!(matches!(
        book.candidate(
            &Operation::SetCellFormula { address, formula },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::MissingRow)
    ));
    let invalid = Formula::new(
        "=A2".into(),
        "=A2".into(),
        BoundAst::new(
            1,
            NodeId(0),
            vec![Node::InvalidReference(ReferenceError::OutOfBounds)],
        )?,
        address,
        DocumentVersion::new(1)?,
    )?;
    assert!(
        book.candidate(
            &Operation::SetCellFormula {
                address,
                formula: invalid
            },
            DocumentVersion::new(2)?
        )
        .is_ok()
    );
    assert_eq!(book.hash(), hash);
    assert_eq!(book.ast_memory_bytes(), 0);
    Ok(())
}

#[test]
fn bound_modes_and_reference_error_codes_are_in_formula_hash()
-> Result<(), Box<dyn std::error::Error>> {
    let (_, address) = fixture()?;
    let reference = BoundCellReference {
        target_sheet: address.sheet,
        target_row: address.row,
        target_column: address.column,
        row_mode: AxisReferenceMode::Absolute,
        column_mode: AxisReferenceMode::Absolute,
    };
    let mut hashes = std::collections::HashSet::new();
    for node in [
        Node::Cell(reference),
        Node::Cell(BoundCellReference {
            row_mode: AxisReferenceMode::Relative { original_delta: 0 },
            ..reference
        }),
        Node::Cell(BoundCellReference {
            column_mode: AxisReferenceMode::Relative { original_delta: 0 },
            ..reference
        }),
        Node::InvalidReference(ReferenceError::DeletedTarget),
        Node::InvalidReference(ReferenceError::MissingSheet),
        Node::InvalidReference(ReferenceError::OutOfBounds),
    ] {
        let formula = Formula::new(
            "=A1".into(),
            "=A1".into(),
            BoundAst::new(1, NodeId(0), vec![node])?,
            address,
            DocumentVersion::new(1)?,
        )?;
        assert!(hashes.insert(formula.semantic_hash()));
    }
    Ok(())
}

#[cfg(feature = "test-support")]
#[test]
fn workbook_ast_budget_rejects_before_mutation_and_clearing_releases_charge()
-> Result<(), Box<dyn std::error::Error>> {
    fn tree(leaves: usize, nodes: &mut Vec<Node>) -> (NodeId, String) {
        if leaves == 1 {
            let id = NodeId(nodes.len() as u32);
            nodes.push(Node::Number(1.0));
            return (id, "1".into());
        }
        let (left, left_text) = tree(leaves / 2, nodes);
        let (right, right_text) = tree(leaves / 2, nodes);
        let id = NodeId(nodes.len() as u32);
        nodes.push(Node::Binary {
            op: BinaryOp::Add,
            left,
            right,
        });
        (id, format!("({left_text}+{right_text})"))
    }
    let mut nodes = Vec::new();
    let (root, source) = tree(2048, &mut nodes);
    let ast = BoundAst::new(1, root, nodes)?;
    let (book, address) = fixture()?;
    let rows = (1u128..=34)
        .map(|id| RowId::from_bytes(id.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let seed = SheetSeed {
        id: address.sheet,
        name: "Sheet".into(),
        rows: rows.clone(),
        columns: vec![address.column],
    };
    let source = format!("={source}");
    let version = DocumentVersion::new(1)?;
    let make = |row| {
        Formula::new(
            source.clone(),
            source.clone(),
            ast.clone(),
            CellAddress { row, ..address },
            version,
        )
    };
    let mut contents = Vec::new();
    for &row in &rows[..32] {
        contents.push((
            CellAddress { row, ..address },
            CellContent::Formula(make(row)?),
        ));
    }
    let book = Workbook::from_content_fixture(
        book.document_id(),
        DocumentVersion::new(1)?,
        seed.clone(),
        contents.clone(),
    )?;
    let charge = make(rows[0])?.ast_memory_bytes();
    assert_eq!(book.ast_memory_bytes(), 32 * charge);
    let before = book.hash();
    let spare = CellAddress {
        row: rows[32],
        ..address
    };
    let operation = Operation::SetCellFormula {
        address: spare,
        formula: make(spare.row)?,
    };
    assert!(matches!(
        book.candidate(&operation, DocumentVersion::new(2)?),
        Err(DomainError::LimitExceeded)
    ));
    assert_eq!(book.hash(), before);
    let occupied = CellAddress {
        row: rows[0],
        ..address
    };
    let writes = vec![
        spreadsheet_domain::workbook::ContentWrite {
            address: spare,
            content: Some(CellContent::Formula(make(spare.row)?)),
        },
        spreadsheet_domain::workbook::ContentWrite {
            address: occupied,
            content: None,
        },
    ];
    let moved = book.candidate(
        &Operation::CopyRange {
            writes: writes.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let reversed = book.candidate(
        &Operation::CopyRange {
            writes: writes.into_iter().rev().collect(),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(moved.hash(), reversed.hash());
    assert_eq!(moved.ast_memory_bytes(), book.ast_memory_bytes());
    assert_eq!(moved.cell_content(occupied)?, None);
    assert_eq!(book.hash(), before);
    contents.push((spare, CellContent::Formula(make(spare.row)?)));
    assert!(matches!(
        Workbook::from_content_fixture(
            book.document_id(),
            DocumentVersion::new(1)?,
            seed,
            contents
        ),
        Err(DomainError::LimitExceeded)
    ));
    let clear = book.candidate(
        &Operation::ClearCell {
            address: CellAddress {
                row: rows[0],
                ..address
            },
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(clear.ast_memory_bytes(), 31 * charge);
    let restored = clear.candidate(&operation, DocumentVersion::new(3)?)?;
    assert_eq!(restored.ast_memory_bytes(), 32 * charge);
    assert_eq!(restored.text_bytes(), book.text_bytes());
    Ok(())
}

#[cfg(feature = "test-support")]
#[test]
fn formula_source_canonical_and_ast_text_share_workbook_text_budget()
-> Result<(), Box<dyn std::error::Error>> {
    let (initial, address) = fixture()?;
    let count = MAX_WORKBOOK_TEXT_BYTES / MAX_TEXT_BYTES;
    let rows = (1..=count + 1)
        .map(|n| RowId::from_bytes((n as u128).to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let cells = rows[..count]
        .iter()
        .map(|&row| {
            (
                CellAddress { row, ..address },
                CellValue::Text("x".repeat(MAX_TEXT_BYTES)),
            )
        })
        .collect::<Vec<_>>();
    let book = Workbook::from_fixture(
        initial.document_id(),
        DocumentVersion::new(1)?,
        SheetSeed {
            id: address.sheet,
            name: "Sheet".into(),
            rows: rows.clone(),
            columns: vec![address.column],
        },
        cells,
    )?;
    let spare = CellAddress {
        row: rows[count],
        ..address
    };
    let formula = Formula::new(
        "=\"x\"".into(),
        "=\"x\"".into(),
        BoundAst::new(1, NodeId(0), vec![Node::Text("x".into())])?,
        spare,
        DocumentVersion::new(1)?,
    )?;
    assert_eq!(formula.text_bytes(), 9);
    let operation = Operation::SetCellFormula {
        address: spare,
        formula,
    };
    assert!(matches!(
        book.candidate(&operation, DocumentVersion::new(2)?),
        Err(DomainError::LimitExceeded)
    ));
    assert_eq!(book.text_bytes(), MAX_WORKBOOK_TEXT_BYTES);
    let cleared = book.candidate(
        &Operation::ClearCell {
            address: CellAddress {
                row: rows[0],
                ..address
            },
        },
        DocumentVersion::new(2)?,
    )?;
    let inserted = cleared.candidate(&operation, DocumentVersion::new(3)?)?;
    assert_eq!(
        inserted.text_bytes(),
        MAX_WORKBOOK_TEXT_BYTES - MAX_TEXT_BYTES + 9
    );
    assert_eq!(inserted.ast_memory_bytes(), 257);
    let replaced = inserted.candidate(
        &Operation::SetCellValue {
            address: spare,
            value: CellValue::Text("xx".into()),
        },
        DocumentVersion::new(4)?,
    )?;
    assert_eq!(
        replaced.text_bytes(),
        MAX_WORKBOOK_TEXT_BYTES - MAX_TEXT_BYTES + 2
    );
    assert_eq!(replaced.ast_memory_bytes(), 0);
    Ok(())
}
