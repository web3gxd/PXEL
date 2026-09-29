use proptest::prelude::*;
use spreadsheet_domain::formula::{
    AST_SCHEMA_VERSION, AxisReferenceMode, BoundAst, BoundCellReference, BoundRangeReference,
    Formula, FunctionName, Node, NodeId, ReferenceError,
};
use spreadsheet_domain::workbook::*;

fn seed() -> Result<(Workbook, CellAddress), DomainError> {
    let document = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let row = RowId::from_bytes([3; 16])?;
    let column = ColumnId::from_bytes([4; 16])?;
    let workbook = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Лист 1".into(),
                rows: vec![row],
                columns: vec![column],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    Ok((workbook, CellAddress { sheet, row, column }))
}

#[test]
fn inserted_column_keeps_bound_target_and_rebases_relative_copy_offset() -> Result<(), DomainError>
{
    let (book, first) = seed()?;
    let second_column = ColumnId::from_bytes([21; 16])?;
    let third_column = ColumnId::from_bytes([22; 16])?;
    let book = book.candidate(
        &Operation::InsertColumnsRebased {
            sheet: first.sheet,
            before: first.column,
            columns: vec![second_column],
        },
        DocumentVersion::new(2)?,
    )?;
    let origin = CellAddress {
        column: first.column,
        ..first
    };
    let reference = BoundCellReference {
        target_sheet: first.sheet,
        target_row: first.row,
        target_column: second_column,
        row_mode: AxisReferenceMode::Relative { original_delta: 0 },
        column_mode: AxisReferenceMode::Relative { original_delta: -1 },
    };
    let ast = BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Cell(reference)])
        .map_err(|_| DomainError::InvalidFormula)?;
    let formula = Formula::new("=A1".into(), "=A1".into(), ast, origin, book.version())
        .map_err(|_| DomainError::InvalidFormula)?;
    let before = book.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula,
        },
        DocumentVersion::new(3)?,
    )?;
    let operation = Operation::InsertColumnsRebased {
        sheet: first.sheet,
        before: first.column,
        columns: vec![third_column],
    };
    let after = before.candidate(&operation, DocumentVersion::new(4)?)?;
    assert_eq!(
        after.sheets()[0].columns(),
        &[second_column, third_column, first.column]
    );
    let CellContent::Formula(rebased) = after.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or(DomainError::InvalidFormula)?
    else {
        return Err(DomainError::InvalidFormula);
    };
    assert!(matches!(&rebased.ast().nodes()[0], Node::Cell(reference)
        if reference.target_column == second_column
        && reference.column_mode == AxisReferenceMode::Relative { original_delta: -2 }));
    assert_eq!(rebased.bound_at_version().get(), 4);
    assert_eq!(after.sheets()[0].column_position(first.column), Some(2));
    let mut replay = before;
    replay.apply_event(&CommittedEvent {
        document_id: after.document_id(),
        operation_id: OperationId::from_bytes([23; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([24; 16])?),
        version: DocumentVersion::new(4)?,
        operation,
        state_hash: after.hash(),
    })?;
    assert_eq!(replay.hash(), after.hash());
    Ok(())
}

#[test]
fn deleted_column_invalidates_scalar_target_and_retired_id_cannot_reappear()
-> Result<(), DomainError> {
    let (book, first) = seed()?;
    let middle = ColumnId::from_bytes([31; 16])?;
    let last = ColumnId::from_bytes([32; 16])?;
    let book = book.candidate(
        &Operation::InsertColumnsRebased {
            sheet: first.sheet,
            before: first.column,
            columns: vec![middle, last],
        },
        DocumentVersion::new(2)?,
    )?;
    let origin = CellAddress {
        column: last,
        ..first
    };
    let ast = BoundAst::new(
        AST_SCHEMA_VERSION,
        NodeId(0),
        vec![Node::Cell(BoundCellReference {
            target_sheet: first.sheet,
            target_row: first.row,
            target_column: middle,
            row_mode: AxisReferenceMode::Relative { original_delta: 0 },
            column_mode: AxisReferenceMode::Relative { original_delta: -1 },
        })],
    )
    .map_err(|_| DomainError::InvalidFormula)?;
    let formula = Formula::new("=A1".into(), "=A1".into(), ast, origin, book.version())
        .map_err(|_| DomainError::InvalidFormula)?;
    let before = book.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula,
        },
        DocumentVersion::new(3)?,
    )?;
    let operation = Operation::DeleteColumns {
        sheet: first.sheet,
        columns: vec![middle],
    };
    let after = before.candidate(&operation, DocumentVersion::new(4)?)?;
    assert_eq!(after.sheets()[0].columns(), &[last, first.column]);
    let CellContent::Formula(rebased) = after.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or(DomainError::InvalidFormula)?
    else {
        return Err(DomainError::InvalidFormula);
    };
    assert_eq!(
        rebased.ast().nodes(),
        &[Node::InvalidReference(ReferenceError::DeletedTarget)]
    );
    assert_eq!(
        after
            .candidate(
                &Operation::InsertColumnsRebased {
                    sheet: first.sheet,
                    before: last,
                    columns: vec![middle],
                },
                DocumentVersion::new(5)?,
            )
            .map(|_| ()),
        Err(DomainError::InvalidAxis)
    );
    assert_eq!(
        after
            .candidate(
                &Operation::SetCellValue {
                    address: CellAddress {
                        column: middle,
                        ..first
                    },
                    value: CellValue::Number(1.0),
                },
                DocumentVersion::new(5)?,
            )
            .map(|_| ()),
        Err(DomainError::MissingColumn)
    );
    let mut replay = before;
    replay.apply_event(&CommittedEvent {
        document_id: after.document_id(),
        operation_id: OperationId::from_bytes([33; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([34; 16])?),
        version: DocumentVersion::new(4)?,
        operation,
        state_hash: after.hash(),
    })?;
    assert_eq!(replay.hash(), after.hash());
    Ok(())
}

#[test]
fn row_extension_preserves_stable_cells_and_replays_atomically() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let filled = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(7.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let first = RowId::from_bytes([7; 16])?;
    let second = RowId::from_bytes([8; 16])?;
    let operation = Operation::ExtendRows {
        sheet: address.sheet,
        rows: vec![first, second],
    };
    let extended = filled.candidate(&operation, DocumentVersion::new(3)?)?;
    assert_eq!(extended.sheets()[0].rows(), &[address.row, first, second]);
    assert_eq!(extended.sheets()[0].row_position(address.row), Some(0));
    assert_eq!(extended.sheets()[0].row_position(second), Some(2));
    assert_eq!(extended.sheets()[0].cells(), filled.sheets()[0].cells());
    assert_eq!(extended.last_axis_change_version().get(), 3);
    assert_eq!(filled.last_axis_change_version().get(), 1);
    assert!(matches!(
        filled.candidate(
            &Operation::ExtendRows {
                sheet: address.sheet,
                rows: vec![first, first],
            },
            DocumentVersion::new(3)?,
        ),
        Err(DomainError::InvalidAxis)
    ));
    let mut replay = filled.clone();
    replay.apply_event(&CommittedEvent {
        document_id: filled.document_id(),
        operation_id: OperationId::from_bytes([9; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([10; 16])?),
        version: DocumentVersion::new(3)?,
        operation,
        state_hash: extended.hash(),
    })?;
    assert_eq!(replay.hash(), extended.hash());
    Ok(())
}

#[test]
fn cell_value_batch_is_atomic_and_replayable() -> Result<(), DomainError> {
    let (book, first) = seed()?;
    let second_row = RowId::from_bytes([55; 16])?;
    let book = book.candidate(
        &Operation::ExtendRows {
            sheet: first.sheet,
            rows: vec![second_row],
        },
        DocumentVersion::new(2)?,
    )?;
    let second = CellAddress {
        row: second_row,
        ..first
    };
    let operation = Operation::SetCellValues {
        writes: vec![
            CellWrite {
                address: first,
                value: CellValue::Number(7.0),
            },
            CellWrite {
                address: second,
                value: CellValue::Text("ok".into()),
            },
        ],
    };
    let after = book.candidate(&operation, DocumentVersion::new(3)?)?;
    assert_eq!(after.version().get(), 3);
    assert_eq!(after.sheets()[0].cells().len(), 2);
    assert_eq!(book.sheets()[0].cells().len(), 0);
    let invalid = Operation::SetCellValues {
        writes: vec![
            CellWrite {
                address: first,
                value: CellValue::Number(9.0),
            },
            CellWrite {
                address: CellAddress {
                    row: RowId::from_bytes([56; 16])?,
                    ..first
                },
                value: CellValue::Number(1.0),
            },
        ],
    };
    assert_eq!(
        book.candidate(&invalid, DocumentVersion::new(3)?)
            .map(|_| ()),
        Err(DomainError::MissingRow)
    );
    assert_eq!(book.sheets()[0].cells().len(), 0);
    let duplicate = Operation::SetCellValues {
        writes: vec![
            CellWrite {
                address: first,
                value: CellValue::Number(1.0),
            },
            CellWrite {
                address: first,
                value: CellValue::Number(2.0),
            },
        ],
    };
    assert_eq!(
        book.candidate(&duplicate, DocumentVersion::new(3)?)
            .map(|_| ()),
        Err(DomainError::InvalidAxis)
    );
    let mut replay = book;
    replay.apply_event(&CommittedEvent {
        document_id: after.document_id(),
        operation_id: OperationId::from_bytes([57; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([58; 16])?),
        version: DocumentVersion::new(3)?,
        operation,
        state_hash: after.hash(),
    })?;
    assert_eq!(replay.hash(), after.hash());
    Ok(())
}

#[test]
fn insert_rows_uses_stable_anchor_and_preserves_existing_cell_identity() -> Result<(), DomainError>
{
    let (book, address) = seed()?;
    let second = RowId::from_bytes([7; 16])?;
    let third = RowId::from_bytes([8; 16])?;
    let expanded = book.candidate(
        &Operation::ExtendRows {
            sheet: address.sheet,
            rows: vec![second, third],
        },
        DocumentVersion::new(2)?,
    )?;
    let filled = expanded.candidate(
        &Operation::SetCellValue {
            address: CellAddress {
                row: second,
                ..address
            },
            value: CellValue::Number(42.0),
        },
        DocumentVersion::new(3)?,
    )?;
    let inserted = RowId::from_bytes([9; 16])?;
    let operation = Operation::InsertRows {
        sheet: address.sheet,
        before: second,
        rows: vec![inserted],
    };
    let next = filled.candidate(&operation, DocumentVersion::new(4)?)?;
    assert_eq!(
        next.sheets()[0].rows(),
        &[address.row, inserted, second, third]
    );
    assert_eq!(next.sheets()[0].row_position(second), Some(2));
    assert_eq!(next.sheets()[0].cells(), filled.sheets()[0].cells());
    assert_eq!(next.last_axis_change_version().get(), 4);
    let mut replay = filled.clone();
    replay.apply_event(&CommittedEvent {
        document_id: filled.document_id(),
        operation_id: OperationId::from_bytes([11; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([12; 16])?),
        version: DocumentVersion::new(4)?,
        operation,
        state_hash: next.hash(),
    })?;
    assert_eq!(replay.hash(), next.hash());
    Ok(())
}

#[test]
fn insert_rows_rejects_formulas_until_reference_transformation_exists() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let ast = BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(1.0)])
        .map_err(|_| DomainError::InvalidFormula)?;
    let formula = Formula::new("=1".into(), "=1".into(), ast, address, book.version())
        .map_err(|_| DomainError::InvalidFormula)?;
    let with_formula = book.candidate(
        &Operation::SetCellFormula { address, formula },
        DocumentVersion::new(2)?,
    )?;
    let original_hash = with_formula.hash();
    assert_eq!(
        with_formula
            .candidate(
                &Operation::InsertRows {
                    sheet: address.sheet,
                    before: address.row,
                    rows: vec![RowId::from_bytes([13; 16])?],
                },
                DocumentVersion::new(3)?,
            )
            .map(|_| ()),
        Err(DomainError::UnsupportedStructuralDependency)
    );
    assert_eq!(with_formula.hash(), original_hash);
    Ok(())
}

#[test]
fn rebased_insertion_preserves_target_and_replays_relative_delta() -> Result<(), DomainError> {
    let (book, first) = seed()?;
    let middle = RowId::from_bytes([21; 16])?;
    let last = RowId::from_bytes([22; 16])?;
    let book = book.candidate(
        &Operation::ExtendRows {
            sheet: first.sheet,
            rows: vec![middle, last],
        },
        DocumentVersion::new(2)?,
    )?;
    let origin = CellAddress { row: last, ..first };
    let reference = BoundCellReference {
        target_sheet: first.sheet,
        target_row: first.row,
        target_column: first.column,
        row_mode: AxisReferenceMode::Relative { original_delta: -2 },
        column_mode: AxisReferenceMode::Relative { original_delta: 0 },
    };
    let ast = BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Cell(reference)])
        .map_err(|_| DomainError::InvalidFormula)?;
    let formula = Formula::new("=A1".into(), "=A1".into(), ast, origin, book.version())
        .map_err(|_| DomainError::InvalidFormula)?;
    let with_formula = book.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula,
        },
        DocumentVersion::new(3)?,
    )?;
    let operation = Operation::InsertRowsRebased {
        sheet: first.sheet,
        before: middle,
        rows: vec![RowId::from_bytes([23; 16])?],
    };
    let next = with_formula.candidate(&operation, DocumentVersion::new(4)?)?;
    let CellContent::Formula(rebased) = next.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or(DomainError::InvalidFormula)?
    else {
        return Err(DomainError::InvalidFormula);
    };
    assert_eq!(rebased.source(), "=A1");
    assert_eq!(rebased.bound_at_version().get(), 4);
    assert!(matches!(&rebased.ast().nodes()[0], Node::Cell(reference)
        if reference.target_row == first.row
        && reference.row_mode == AxisReferenceMode::Relative { original_delta: -3 }));
    let mut replay = with_formula;
    replay.apply_event(&CommittedEvent {
        document_id: next.document_id(),
        operation_id: OperationId::from_bytes([24; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([25; 16])?),
        version: DocumentVersion::new(4)?,
        operation,
        state_hash: next.hash(),
    })?;
    assert_eq!(replay.hash(), next.hash());
    Ok(())
}

#[test]
fn rebase_uses_target_sheet_axis_for_cross_sheet_reference() -> Result<(), DomainError> {
    let (_, origin) = seed()?;
    let target_sheet = SheetId::from_bytes([31; 16])?;
    let target_row = RowId::from_bytes([32; 16])?;
    let target_column = ColumnId::from_bytes([33; 16])?;
    let reference = BoundCellReference {
        target_sheet,
        target_row,
        target_column,
        row_mode: AxisReferenceMode::Relative { original_delta: 1 },
        column_mode: AxisReferenceMode::Absolute,
    };
    let ast = BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Cell(reference)])
        .map_err(|_| DomainError::InvalidFormula)?;
    let formula = Formula::new(
        "='Other'!$A2".into(),
        "='Other'!$A2".into(),
        ast,
        origin,
        DocumentVersion::new(1)?,
    )
    .map_err(|_| DomainError::InvalidFormula)?;
    let rebased = formula
        .rebase_row_offsets(DocumentVersion::new(2)?, 2, |sheet, row| {
            (sheet == target_sheet && row == target_row).then_some(4)
        })
        .map_err(|_| DomainError::InvalidFormula)?;
    assert!(matches!(&rebased.ast().nodes()[0], Node::Cell(reference)
        if reference.target_sheet == target_sheet
        && reference.target_row == target_row
        && reference.row_mode == AxisReferenceMode::Relative { original_delta: 2 }
        && reference.column_mode == AxisReferenceMode::Absolute));
    Ok(())
}

#[test]
fn delete_rows_invalidates_scalar_target_and_prevents_id_reuse() -> Result<(), DomainError> {
    let (book, first) = seed()?;
    let middle = RowId::from_bytes([41; 16])?;
    let last = RowId::from_bytes([42; 16])?;
    let book = book.candidate(
        &Operation::ExtendRows {
            sheet: first.sheet,
            rows: vec![middle, last],
        },
        DocumentVersion::new(2)?,
    )?;
    let filled = book.candidate(
        &Operation::SetCellValue {
            address: CellAddress {
                row: middle,
                ..first
            },
            value: CellValue::Number(10.0),
        },
        DocumentVersion::new(3)?,
    )?;
    let origin = CellAddress { row: last, ..first };
    let reference = BoundCellReference {
        target_sheet: first.sheet,
        target_row: middle,
        target_column: first.column,
        row_mode: AxisReferenceMode::Relative { original_delta: -1 },
        column_mode: AxisReferenceMode::Relative { original_delta: 0 },
    };
    let ast = BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Cell(reference)])
        .map_err(|_| DomainError::InvalidFormula)?;
    let formula = Formula::new("=A2".into(), "=A2".into(), ast, origin, filled.version())
        .map_err(|_| DomainError::InvalidFormula)?;
    let before = filled.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula,
        },
        DocumentVersion::new(4)?,
    )?;
    let operation = Operation::DeleteRows {
        sheet: first.sheet,
        rows: vec![middle],
    };
    let after = before.candidate(&operation, DocumentVersion::new(5)?)?;
    assert_eq!(after.sheets()[0].rows(), &[first.row, last]);
    assert!(!after.sheets()[0].cells().contains_key(&CellAddress {
        row: middle,
        ..first
    }));
    let CellContent::Formula(rebased) = after.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or(DomainError::InvalidFormula)?
    else {
        return Err(DomainError::InvalidFormula);
    };
    assert_eq!(
        rebased.ast().nodes(),
        &[Node::InvalidReference(ReferenceError::DeletedTarget)]
    );
    assert_eq!(rebased.bound_at_version().get(), 5);
    assert_eq!(after.text_bytes(), rebased.text_bytes());
    assert_eq!(
        after
            .candidate(
                &Operation::SetCellValue {
                    address: CellAddress {
                        row: middle,
                        ..first
                    },
                    value: CellValue::Number(2.0)
                },
                DocumentVersion::new(6)?,
            )
            .map(|_| ()),
        Err(DomainError::MissingRow)
    );
    assert_eq!(
        after
            .candidate(
                &Operation::InsertRowsRebased {
                    sheet: first.sheet,
                    before: last,
                    rows: vec![middle]
                },
                DocumentVersion::new(6)?,
            )
            .map(|_| ()),
        Err(DomainError::InvalidAxis)
    );
    let mut replay = before;
    replay.apply_event(&CommittedEvent {
        document_id: after.document_id(),
        operation_id: OperationId::from_bytes([43; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([44; 16])?),
        version: DocumentVersion::new(5)?,
        operation,
        state_hash: after.hash(),
    })?;
    assert_eq!(replay.hash(), after.hash());
    Ok(())
}

#[test]
fn delete_rows_shrinks_range_boundary_and_rejects_last_row() -> Result<(), DomainError> {
    let (book, first) = seed()?;
    let middle = RowId::from_bytes([51; 16])?;
    let last = RowId::from_bytes([52; 16])?;
    let book = book.candidate(
        &Operation::ExtendRows {
            sheet: first.sheet,
            rows: vec![middle, last],
        },
        DocumentVersion::new(2)?,
    )?;
    let origin = CellAddress { row: last, ..first };
    let reference = |row, delta| BoundCellReference {
        target_sheet: first.sheet,
        target_row: row,
        target_column: first.column,
        row_mode: AxisReferenceMode::Relative {
            original_delta: delta,
        },
        column_mode: AxisReferenceMode::Relative { original_delta: 0 },
    };
    let ast = BoundAst::new(
        AST_SCHEMA_VERSION,
        NodeId(1),
        vec![
            Node::Range(BoundRangeReference {
                start: reference(first.row, -2),
                end: reference(middle, -1),
            }),
            Node::Call {
                name: FunctionName::new("SUM".into()).map_err(|_| DomainError::InvalidFormula)?,
                arguments: vec![NodeId(0)],
            },
        ],
    )
    .map_err(|_| DomainError::InvalidFormula)?;
    let formula = Formula::new(
        "=SUM(A1:A2)".into(),
        "=SUM(A1:A2)".into(),
        ast,
        origin,
        book.version(),
    )
    .map_err(|_| DomainError::InvalidFormula)?;
    let before = book.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula,
        },
        DocumentVersion::new(3)?,
    )?;
    let after = before.candidate(
        &Operation::DeleteRows {
            sheet: first.sheet,
            rows: vec![first.row],
        },
        DocumentVersion::new(4)?,
    )?;
    let CellContent::Formula(rebased) = after.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or(DomainError::InvalidFormula)?
    else {
        return Err(DomainError::InvalidFormula);
    };
    assert!(matches!(&rebased.ast().nodes()[0], Node::Range(range)
        if range.start.target_row == middle
        && range.end.target_row == middle
        && range.start.row_mode == AxisReferenceMode::Relative { original_delta: -1 }));
    let only_last = after.candidate(
        &Operation::DeleteRows {
            sheet: first.sheet,
            rows: vec![middle],
        },
        DocumentVersion::new(5)?,
    )?;
    let CellContent::Formula(invalidated) = only_last.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or(DomainError::InvalidFormula)?
    else {
        return Err(DomainError::InvalidFormula);
    };
    assert!(matches!(
        invalidated.ast().nodes()[0],
        Node::InvalidReference(ReferenceError::DeletedTarget)
    ));
    assert_eq!(
        only_last
            .candidate(
                &Operation::DeleteRows {
                    sheet: first.sheet,
                    rows: vec![last]
                },
                DocumentVersion::new(6)?,
            )
            .map(|_| ()),
        Err(DomainError::InvalidAxis)
    );
    Ok(())
}

#[test]
fn cells_are_sparse_and_blank_removes_storage() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    assert!(book.sheets()[0].cells().is_empty());
    let updated = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(42.0),
        },
        DocumentVersion::new(2)?,
    )?;
    assert!(book.sheets()[0].cells().is_empty());
    assert_eq!(
        updated.sheets()[0].cells().get(&address),
        Some(&CellContent::Value(CellValue::Number(42.0)))
    );
    let cleared = updated.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Blank,
        },
        DocumentVersion::new(3)?,
    )?;
    assert!(cleared.sheets()[0].cells().is_empty());
    assert_eq!(cleared.sheets()[0].rows(), book.sheets()[0].rows());
    Ok(())
}

#[test]
fn corrupt_or_duplicate_replay_never_mutates_projection() -> Result<(), DomainError> {
    let (mut book, address) = seed()?;
    let original = book.hash();
    let operation = Operation::SetCellValue {
        address,
        value: CellValue::Boolean(true),
    };
    let candidate = book.candidate(&operation, DocumentVersion::new(2)?)?;
    let mut event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: candidate.version(),
        operation,
        state_hash: [0; 32],
    };
    assert_eq!(book.apply_event(&event), Err(DomainError::HashMismatch));
    assert_eq!(book.hash(), original);
    event.state_hash = candidate.hash();
    book.apply_event(&event)?;
    assert_eq!(book.hash(), candidate.hash());
    assert_eq!(book.apply_event(&event), Err(DomainError::VersionConflict));
    assert_eq!(book.hash(), candidate.hash());
    Ok(())
}

#[test]
fn invalid_inputs_do_not_enter_candidate() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            CellValue::Number(number).validate(),
            Err(DomainError::InvalidValue)
        );
    }
    assert_eq!(
        CellValue::Text("я".repeat(MAX_TEXT_BYTES)).validate(),
        Err(DomainError::LimitExceeded)
    );
    assert_eq!(DocumentId::from_bytes([0; 16]), Err(DomainError::InvalidId));
    assert_eq!(
        DocumentVersion::new(u64::MAX),
        Err(DomainError::LimitExceeded)
    );
    let invalid_row = CellAddress {
        row: RowId::from_bytes([9; 16])?,
        ..address
    };
    assert!(matches!(
        book.candidate(
            &Operation::ClearCell {
                address: invalid_row
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::MissingRow)
    ));
    let invalid_column = CellAddress {
        column: ColumnId::from_bytes([9; 16])?,
        ..address
    };
    assert!(matches!(
        book.candidate(
            &Operation::ClearCell {
                address: invalid_column
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::MissingColumn)
    ));
    let invalid_sheet = CellAddress {
        sheet: SheetId::from_bytes([9; 16])?,
        ..address
    };
    assert!(matches!(
        book.candidate(
            &Operation::ClearCell {
                address: invalid_sheet
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::MissingSheet)
    ));
    assert!(matches!(
        book.candidate(&Operation::ClearCell { address }, DocumentVersion::new(3)?),
        Err(DomainError::VersionConflict)
    ));
    let duplicate = SheetSeed {
        id: address.sheet,
        name: "Sheet".into(),
        rows: vec![address.row, address.row],
        columns: vec![address.column],
    };
    assert!(matches!(
        Workbook::empty(book.document_id()).candidate(
            &Operation::CreateWorkbook { sheet: duplicate },
            DocumentVersion::new(1)?
        ),
        Err(DomainError::InvalidAxis)
    ));
    Ok(())
}

#[test]
fn negative_zero_is_canonical_and_debug_redacts_content() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let negative = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(-0.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let positive = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(0.0),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(negative.hash(), positive.hash());
    assert_eq!(
        format!("{:?}", CellValue::Text("secret-user-content".into())),
        "Text([redacted])"
    );
    assert_eq!(
        format!("{:?}", CellValue::Number(12.125)),
        "Number([redacted])"
    );
    assert_eq!(
        format!("{:?}", CellValue::Boolean(true)),
        "Boolean([redacted])"
    );
    Ok(())
}

#[test]
fn canonical_hash_matches_independently_encoded_golden() -> Result<(), DomainError> {
    // Independently encoded with Node crypto.createHash("sha256") from the
    // documented v1 byte format, without Rust/protobuf serialization.
    let (book, _) = seed()?;
    let actual = book.hash().map(|byte| format!("{byte:02x}")).concat();
    assert_eq!(
        actual,
        "0075d48b9879a8214bec3b5e6488575d1cfec07c9776aae4ec92fa39a76e2ccd"
    );
    Ok(())
}

#[test]
fn hash_distinguishes_types_and_replay_rejects_other_document() -> Result<(), DomainError> {
    let (mut book, address) = seed()?;
    let mut hashes = std::collections::HashSet::new();
    for value in [
        CellValue::Blank,
        CellValue::Text(String::new()),
        CellValue::Text("1".into()),
        CellValue::Number(1.0),
        CellValue::Boolean(true),
    ] {
        let next = book.candidate(
            &Operation::SetCellValue { address, value },
            DocumentVersion::new(2)?,
        )?;
        assert!(hashes.insert(next.hash()));
    }
    let before = book.hash();
    let event = CommittedEvent {
        document_id: DocumentId::from_bytes([7; 16])?,
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: DocumentVersion::new(2)?,
        operation: Operation::ClearCell { address },
        state_hash: before,
    };
    assert_eq!(book.apply_event(&event), Err(DomainError::InvalidId));
    assert_eq!(book.hash(), before);
    Ok(())
}

#[test]
fn duplicate_and_oversized_axes_are_rejected() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let empty = Workbook::empty(book.document_id());
    for (rows, columns, expected) in [
        (
            vec![address.row],
            vec![address.column, address.column],
            DomainError::InvalidAxis,
        ),
        (
            vec![address.row; MAX_ROWS + 1],
            vec![address.column],
            DomainError::LimitExceeded,
        ),
        (
            vec![address.row],
            vec![address.column; MAX_COLUMNS + 1],
            DomainError::LimitExceeded,
        ),
        (vec![], vec![address.column], DomainError::InvalidAxis),
    ] {
        let operation = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: address.sheet,
                name: "Sheet".into(),
                rows,
                columns,
            },
        };
        assert!(
            matches!(empty.candidate(&operation, DocumentVersion::new(1)?), Err(actual) if actual == expected)
        );
        assert!(empty.sheets().is_empty());
    }
    Ok(())
}

proptest! {
    #[test]
    fn inserting_before_any_anchor_preserves_existing_id_positions(
        row_count in 1usize..64,
        raw_anchor in any::<u16>(),
        value in any::<i32>(),
    ) {
        let sheet = SheetId::from_bytes([2;16])?;
        let column = ColumnId::from_bytes([3;16])?;
        let rows = (1..=row_count)
            .map(|index| RowId::from_bytes((index as u128).to_be_bytes()))
            .collect::<Result<Vec<_>, _>>()?;
        let anchor_index = usize::from(raw_anchor) % row_count;
        let anchor = rows[anchor_index];
        let address = CellAddress { sheet, row: anchor, column };
        let book = Workbook::empty(DocumentId::from_bytes([1;16])?).candidate(
            &Operation::CreateWorkbook { sheet: SheetSeed {
                id: sheet, name: "Sheet".into(), rows: rows.clone(), columns: vec![column],
            } },
            DocumentVersion::new(1)?,
        )?.candidate(
            &Operation::SetCellValue { address, value: CellValue::Number(f64::from(value)) },
            DocumentVersion::new(2)?,
        )?;
        let inserted = RowId::from_bytes(((row_count + 1) as u128).to_be_bytes())?;
        let next = book.candidate(
            &Operation::InsertRows { sheet, before: anchor, rows: vec![inserted] },
            DocumentVersion::new(3)?,
        )?;
        let mut expected = rows;
        expected.insert(anchor_index, inserted);
        prop_assert_eq!(next.sheets()[0].rows(), expected.as_slice());
        for (position, id) in expected.iter().copied().enumerate() {
            prop_assert_eq!(next.sheets()[0].row_position(id), Some(position as u32));
        }
        prop_assert_eq!(
            next.sheets()[0].cells().get(&address),
            Some(&CellContent::Value(CellValue::Number(f64::from(value)))),
        );
    }

    #[test]
    fn independent_writes_converge_in_canonical_hash(a in any::<i32>(), b in any::<bool>()) {
        let document = DocumentId::from_bytes([1;16])?;
        let sheet = SheetId::from_bytes([2;16])?;
        let row_a = RowId::from_bytes([3;16])?;
        let row_b = RowId::from_bytes([4;16])?;
        let column = ColumnId::from_bytes([5;16])?;
        let book = Workbook::empty(document).candidate(&Operation::CreateWorkbook { sheet: SheetSeed { id: sheet, name: "Sheet".into(), rows: vec![row_b,row_a], columns: vec![column] } }, DocumentVersion::new(1)?)?;
        let first = Operation::SetCellValue { address: CellAddress { sheet, row: row_a, column }, value: CellValue::Number(f64::from(a)) };
        let second = Operation::SetCellValue { address: CellAddress { sheet, row: row_b, column }, value: CellValue::Boolean(b) };
        let left = book.candidate(&first, DocumentVersion::new(2)?)?.candidate(&second, DocumentVersion::new(3)?)?;
        let right = book.candidate(&second, DocumentVersion::new(2)?)?.candidate(&first, DocumentVersion::new(3)?)?;
        prop_assert_eq!(left.hash(), right.hash());
        prop_assert_eq!(left.sheets()[0].rows(), &[row_b,row_a]);
    }

    #[test]
    fn values_are_finite_and_zero_normalized(number in any::<f64>()) {
        let result = CellValue::Number(number).normalized();
        if number.is_finite() {
            let CellValue::Number(normalized) = result? else { return Err(TestCaseError::fail("number type changed")); };
            prop_assert!(normalized.is_finite());
            if normalized == 0.0 { prop_assert_eq!(normalized.to_bits(), 0); }
        } else { prop_assert_eq!(result, Err(DomainError::InvalidValue)); }
    }

    #[test]
    fn cached_text_budget_matches_contents_after_replacements(values in prop::collection::vec("(?:x|я){0,10}", 0..50)) {
        let (mut book, address) = seed()?;
        for (index, text) in values.into_iter().enumerate() {
            let operation = match index % 4 {
                0 | 1 => Operation::SetCellValue { address, value: CellValue::Text(text) },
                2 => Operation::SetCellValue { address, value: CellValue::Number(1.0) },
                _ => Operation::ClearCell { address },
            };
            book = book.candidate(&operation, DocumentVersion::new(book.version().get()+1)?)?;
            let actual: usize = book.sheets().iter().flat_map(|sheet| sheet.cells().values()).map(|value| match value { CellContent::Value(CellValue::Text(text)) => text.len(), _ => 0 }).sum();
            prop_assert_eq!(book.text_bytes(), actual);
        }
    }
}

#[cfg(feature = "test-support")]
#[test]
fn bulk_fixture_validates_and_matches_real_operations() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let sheet = &book.sheets()[0];
    let seed = SheetSeed {
        id: sheet.id(),
        name: sheet.name().into(),
        rows: sheet.rows().into(),
        columns: sheet.columns().into(),
    };
    let fixture = Workbook::from_fixture(
        book.document_id(),
        DocumentVersion::new(2)?,
        seed.clone(),
        [(address, CellValue::Number(-0.0))],
    )?;
    let replay = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(0.0),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(fixture.hash(), replay.hash());
    assert!(matches!(
        Workbook::from_fixture(
            book.document_id(),
            DocumentVersion::new(2)?,
            seed,
            [(address, CellValue::Number(f64::NAN))]
        ),
        Err(DomainError::InvalidValue)
    ));
    Ok(())
}

#[cfg(feature = "test-support")]
#[test]
fn hundred_thousand_sparse_cells_are_bounded_and_clearable() -> Result<(), DomainError> {
    let document_id = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let rows = (1..=MAX_ROWS)
        .map(|n| RowId::from_bytes((n as u128).to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let columns = vec![
        ColumnId::from_bytes([3; 16])?,
        ColumnId::from_bytes([4; 16])?,
    ];
    let address = CellAddress {
        sheet,
        row: rows[0],
        column: columns[1],
    };
    let cells = rows
        .iter()
        .copied()
        .map(|row| {
            (
                CellAddress {
                    sheet,
                    row,
                    column: columns[0],
                },
                CellValue::Boolean(true),
            )
        })
        .collect::<Vec<_>>();
    let book = Workbook::from_fixture(
        document_id,
        DocumentVersion::new(1)?,
        SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows,
            columns,
        },
        cells,
    )?;
    assert_eq!(book.sheets()[0].cells().len(), MAX_CELLS);
    assert_eq!(book.sheets()[0].row_position(address.row), Some(0));
    assert_eq!(book.sheets()[0].column_position(address.column), Some(1));
    assert!(matches!(
        book.candidate(
            &Operation::SetCellValue {
                address,
                value: CellValue::Boolean(false)
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::LimitExceeded)
    ));
    let filled = CellAddress {
        column: book.sheets()[0].columns()[0],
        ..address
    };
    let next = book.candidate(
        &Operation::ClearCell { address: filled },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(next.sheets()[0].cells().len(), MAX_CELLS - 1);
    let original_hash = book.hash();
    assert_ne!(next.hash(), original_hash);
    Ok(())
}

#[cfg(feature = "test-support")]
#[test]
fn workbook_text_budget_is_atomic_and_recovered_by_replacement_and_clear() -> Result<(), DomainError>
{
    let document = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let column = ColumnId::from_bytes([3; 16])?;
    let count = 16 * 1024 * 1024 / MAX_TEXT_BYTES;
    let rows = (1..=count + 1)
        .map(|n| RowId::from_bytes((n as u128).to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let first = CellAddress {
        sheet,
        row: rows[0],
        column,
    };
    let spare = CellAddress {
        sheet,
        row: rows[count],
        column,
    };
    let values = rows[..count]
        .iter()
        .map(|&row| {
            (
                CellAddress { sheet, row, column },
                CellValue::Text("x".repeat(MAX_TEXT_BYTES)),
            )
        })
        .collect::<Vec<_>>();
    let seed = SheetSeed {
        id: sheet,
        name: "Sheet".into(),
        rows,
        columns: vec![column],
    };
    let book = Workbook::from_fixture(
        document,
        DocumentVersion::new(1)?,
        seed.clone(),
        values.clone(),
    )?;
    assert_eq!(book.text_bytes(), 16 * 1024 * 1024);
    let original = book.hash();
    assert!(matches!(
        book.candidate(
            &Operation::SetCellValue {
                address: spare,
                value: CellValue::Text("я".into())
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::LimitExceeded)
    ));
    assert_eq!(book.hash(), original);
    assert_eq!(book.text_bytes(), 16 * 1024 * 1024);
    let reduced = book.candidate(
        &Operation::SetCellValue {
            address: first,
            value: CellValue::Text("я".into()),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(reduced.text_bytes(), 16 * 1024 * 1024 - MAX_TEXT_BYTES + 2);
    let filled = reduced.candidate(
        &Operation::SetCellValue {
            address: spare,
            value: CellValue::Text("x".repeat(MAX_TEXT_BYTES - 2)),
        },
        DocumentVersion::new(3)?,
    )?;
    assert_eq!(filled.text_bytes(), 16 * 1024 * 1024);
    let cleared = filled.candidate(
        &Operation::ClearCell { address: first },
        DocumentVersion::new(4)?,
    )?;
    assert_eq!(cleared.text_bytes(), 16 * 1024 * 1024 - 2);
    let added = cleared.candidate(
        &Operation::SetCellValue {
            address: first,
            value: CellValue::Text("я".into()),
        },
        DocumentVersion::new(5)?,
    )?;
    assert_eq!(added.text_bytes(), 16 * 1024 * 1024);
    let removed = added.candidate(
        &Operation::SetCellValue {
            address: first,
            value: CellValue::Blank,
        },
        DocumentVersion::new(6)?,
    )?;
    assert_eq!(removed.text_bytes(), 16 * 1024 * 1024 - 2);
    let mut excess = values;
    excess.push((spare, CellValue::Text("x".into())));
    assert!(matches!(
        Workbook::from_fixture(document, DocumentVersion::new(1)?, seed, excess),
        Err(DomainError::LimitExceeded)
    ));
    Ok(())
}
