use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};
use spreadsheet_domain::workbook::{
    ActorId, CalculationContext, CalculationContextId, CalculationReason, CellAddress, CellValue,
    ColumnId, CommittedEvent, DocumentId, DocumentVersion, DomainError, EventPrincipal, Operation,
    OperationId, RowId, SheetId, SheetSeed, Workbook,
};

fn version(value: u64) -> Result<DocumentVersion, DomainError> {
    DocumentVersion::new(value)
}

#[test]
fn snapshot_restores_more_than_one_sheet_content_capacity() -> Result<(), DomainError> {
    fn stable(prefix: u8, index: u32) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[0] = prefix;
        bytes[12..].copy_from_slice(&index.to_be_bytes());
        bytes
    }
    let mut seeds = Vec::new();
    let mut cells = Vec::new();
    for prefix in [10u8, 20] {
        let sheet = SheetId::from_bytes(stable(prefix, 0))?;
        let column = ColumnId::from_bytes(stable(prefix + 1, 0))?;
        let rows = (0..50_001)
            .map(|index| RowId::from_bytes(stable(prefix + 2, index)))
            .collect::<Result<Vec<_>, _>>()?;
        cells.extend(rows.iter().map(|row| {
            (
                CellAddress {
                    sheet,
                    row: *row,
                    column,
                },
                CellValue::Number(1.).into(),
            )
        }));
        seeds.push(SheetSeed {
            id: sheet,
            name: format!("Sheet{prefix}"),
            rows,
            columns: vec![column],
        });
    }
    let book =
        Workbook::hydrate_sheets(DocumentId::from_bytes([1; 16])?, version(1)?, seeds, cells)?;
    let restored = Workbook::restore_snapshot(book.snapshot_image()?, book.hash())?;
    assert_eq!(restored.hash(), book.hash());
    assert_eq!(
        restored
            .sheets()
            .iter()
            .map(|s| s.cells().len())
            .sum::<usize>(),
        100_002
    );
    Ok(())
}

#[test]
fn snapshot_restores_retired_axes_context_and_replays_tail() -> Result<(), DomainError> {
    let document = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let rows = [RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?];
    let columns = [
        ColumnId::from_bytes([5; 16])?,
        ColumnId::from_bytes([6; 16])?,
    ];
    let inserted_row = RowId::from_bytes([7; 16])?;
    let inserted_column = ColumnId::from_bytes([8; 16])?;
    let mut book = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet".into(),
                rows: rows.to_vec(),
                columns: columns.to_vec(),
            },
        },
        version(1)?,
    )?;
    book = book.candidate(
        &Operation::SetCellValue {
            address: CellAddress {
                sheet,
                row: rows[0],
                column: columns[0],
            },
            value: CellValue::Number(7.0),
        },
        version(2)?,
    )?;
    book = book.candidate(
        &Operation::InsertRows {
            sheet,
            before: rows[1],
            rows: vec![inserted_row],
        },
        version(3)?,
    )?;
    book = book.candidate(
        &Operation::DeleteRows {
            sheet,
            rows: vec![rows[1]],
        },
        version(4)?,
    )?;
    book = book.candidate(
        &Operation::InsertColumnsRebased {
            sheet,
            before: columns[1],
            columns: vec![inserted_column],
        },
        version(5)?,
    )?;
    book = book.candidate(
        &Operation::DeleteColumns {
            sheet,
            columns: vec![columns[1]],
        },
        version(6)?,
    )?;
    let formula_address = CellAddress {
        sheet,
        row: inserted_row,
        column: inserted_column,
    };
    book = book.candidate(
        &Operation::SetCellFormula {
            address: formula_address,
            formula: Formula::new(
                "=42".into(),
                "=42".into(),
                BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(42.0)])
                    .map_err(|_| DomainError::InvalidFormula)?,
                formula_address,
                version(6)?,
            )
            .map_err(|_| DomainError::InvalidFormula)?,
        },
        version(7)?,
    )?;
    let context = CalculationContext::new(
        CalculationContextId::from_bytes([9; 16])?,
        1,
        86_400_000,
        0,
        1,
        [10; 32],
        "engine-v1".into(),
        "pxel-excel-subset-v1".into(),
        CalculationReason::Explicit,
    )?;
    book = book.candidate(&Operation::AdvanceCalculationEpoch { context }, version(8)?)?;
    let image = book.snapshot_image()?;
    assert_eq!(image.last_axis_change_version, version(6)?);
    assert_eq!(image.retired_rows, vec![rows[1]]);
    assert_eq!(
        book.retired_row_ids().collect::<Vec<_>>(),
        image.retired_rows
    );
    assert_eq!(image.retired_columns, vec![columns[1]]);
    assert_eq!(
        book.retired_column_ids().collect::<Vec<_>>(),
        image.retired_columns
    );
    let mut restored = Workbook::restore_snapshot(image.clone(), book.hash())?;
    assert_eq!(restored.hash(), book.hash());
    assert_eq!(
        restored.cell_content(formula_address)?,
        book.cell_content(formula_address)?
    );
    assert_eq!(restored.calculation_context(), book.calculation_context());
    assert_eq!(
        restored.last_axis_change_version(),
        book.last_axis_change_version()
    );
    let change = Operation::SetCellValue {
        address: CellAddress {
            sheet,
            row: inserted_row,
            column: inserted_column,
        },
        value: CellValue::Number(11.0),
    };
    let direct = book.candidate(&change, version(9)?)?;
    restored.apply_event(&CommittedEvent {
        document_id: document,
        operation_id: OperationId::from_bytes([11; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([12; 16])?),
        version: version(9)?,
        operation: change,
        state_hash: direct.hash(),
    })?;
    assert_eq!(restored.hash(), direct.hash());
    assert_eq!(
        restored
            .candidate(
                &Operation::InsertRows {
                    sheet,
                    before: rows[0],
                    rows: vec![rows[1]],
                },
                version(10)?,
            )
            .err(),
        Some(DomainError::InvalidAxis)
    );
    let mut overlap = image.clone();
    overlap.retired_rows.push(rows[0]);
    assert!(matches!(
        Workbook::restore_snapshot(overlap, book.hash()),
        Err(DomainError::InvalidAxis)
    ));
    let mut duplicate = image;
    duplicate.retired_rows.push(rows[1]);
    assert!(matches!(
        Workbook::restore_snapshot(duplicate, book.hash()),
        Err(DomainError::InvalidAxis)
    ));
    assert!(matches!(
        Workbook::restore_snapshot(book.snapshot_image()?, [0; 32]),
        Err(DomainError::HashMismatch)
    ));
    Ok(())
}
