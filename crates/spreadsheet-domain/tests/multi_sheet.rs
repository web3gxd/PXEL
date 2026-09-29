use spreadsheet_domain::workbook::{
    CellAddress, CellValue, ColumnId, DocumentId, DocumentVersion, DomainError, Operation, RowId,
    SheetId, SheetSeed, Workbook,
};

fn version(value: u64) -> Result<DocumentVersion, DomainError> {
    DocumentVersion::new(value)
}

#[test]
fn second_sheet_has_stable_axes_and_survives_snapshot_replay() -> Result<(), DomainError> {
    let document = DocumentId::from_bytes([1; 16])?;
    let first = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Budget".into(),
        rows: vec![RowId::from_bytes([3; 16])?],
        columns: vec![ColumnId::from_bytes([4; 16])?],
    };
    let second = SheetSeed {
        id: SheetId::from_bytes([5; 16])?,
        name: "Plan".into(),
        rows: vec![RowId::from_bytes([6; 16])?],
        columns: vec![ColumnId::from_bytes([7; 16])?],
    };
    let mut book = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: first.clone(),
        },
        version(1)?,
    )?;
    book = book.candidate(
        &Operation::AddSheet {
            sheet: second.clone(),
        },
        version(2)?,
    )?;
    let first_address = CellAddress {
        sheet: first.id,
        row: first.rows[0],
        column: first.columns[0],
    };
    let second_address = CellAddress {
        sheet: second.id,
        row: second.rows[0],
        column: second.columns[0],
    };
    book = book.candidate(
        &Operation::SetCellValue {
            address: first_address,
            value: CellValue::Number(7.0),
        },
        version(3)?,
    )?;
    book = book.candidate(
        &Operation::SetCellValue {
            address: second_address,
            value: CellValue::Text("different sheet".into()),
        },
        version(4)?,
    )?;
    assert_eq!(book.sheets().len(), 2);
    assert_eq!(book.last_axis_change_version(), version(2)?);
    let restored = Workbook::restore_snapshot(book.snapshot_image()?, book.hash())?;
    assert_eq!(restored.hash(), book.hash());
    assert_eq!(
        restored.cell_content(first_address)?,
        book.cell_content(first_address)?
    );
    assert_eq!(
        restored.cell_content(second_address)?,
        book.cell_content(second_address)?
    );

    let duplicate_row = SheetSeed {
        id: SheetId::from_bytes([8; 16])?,
        name: "Other".into(),
        rows: first.rows.clone(),
        columns: vec![ColumnId::from_bytes([9; 16])?],
    };
    assert!(matches!(
        book.candidate(
            &Operation::AddSheet {
                sheet: duplicate_row
            },
            version(5)?
        ),
        Err(DomainError::InvalidAxis)
    ));
    let duplicate_name = SheetSeed {
        id: SheetId::from_bytes([10; 16])?,
        name: "budget".into(),
        rows: vec![RowId::from_bytes([11; 16])?],
        columns: vec![ColumnId::from_bytes([12; 16])?],
    };
    assert!(matches!(
        book.candidate(
            &Operation::AddSheet {
                sheet: duplicate_name
            },
            version(5)?
        ),
        Err(DomainError::InvalidAxis)
    ));
    let mut corrupt = book.snapshot_image()?;
    corrupt.sheets[1].sheet.rows[0] = first.rows[0];
    assert!(matches!(
        Workbook::restore_snapshot(corrupt, book.hash()),
        Err(DomainError::InvalidAxis)
    ));
    Ok(())
}
