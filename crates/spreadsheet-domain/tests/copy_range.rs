use spreadsheet_domain::workbook::{
    CellAddress, CellContent, CellValue, ColumnId, ContentWrite, DocumentId, DocumentVersion,
    DomainError, Operation, RowId, SheetId, SheetSeed, Workbook,
};

#[test]
fn frozen_copy_is_atomic_and_does_not_depend_on_source_after_commit() -> Result<(), DomainError> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let row = RowId::from_bytes([2; 16])?;
    let source = ColumnId::from_bytes([3; 16])?;
    let target = ColumnId::from_bytes([4; 16])?;
    let seed = SheetSeed {
        id: sheet,
        name: "Sheet".into(),
        rows: vec![row],
        columns: vec![source, target],
    };
    let book = Workbook::empty(DocumentId::from_bytes([5; 16])?).candidate(
        &Operation::CreateWorkbook { sheet: seed },
        DocumentVersion::new(1)?,
    )?;
    let source_address = CellAddress {
        sheet,
        row,
        column: source,
    };
    let target_address = CellAddress {
        sheet,
        row,
        column: target,
    };
    let book = book.candidate(
        &Operation::SetCellValue {
            address: source_address,
            value: CellValue::Number(3.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let copy = Operation::CopyRange {
        writes: vec![ContentWrite {
            address: target_address,
            content: Some(CellContent::Value(CellValue::Number(3.0))),
        }],
    };
    let copied = book.candidate(&copy, DocumentVersion::new(3)?)?;
    assert_eq!(
        copied.cell_content(target_address)?,
        copied.cell_content(source_address)?
    );
    let changed_source = copied.candidate(
        &Operation::SetCellValue {
            address: source_address,
            value: CellValue::Number(9.0),
        },
        DocumentVersion::new(4)?,
    )?;
    assert_eq!(
        changed_source.cell_content(target_address)?,
        Some(&CellContent::Value(CellValue::Number(3.0)))
    );
    let duplicate = Operation::CopyRange {
        writes: vec![
            ContentWrite {
                address: target_address,
                content: None,
            },
            ContentWrite {
                address: target_address,
                content: None,
            },
        ],
    };
    assert_eq!(
        book.candidate(&duplicate, DocumentVersion::new(3)?).err(),
        Some(DomainError::InvalidAxis)
    );
    assert_eq!(book.version().get(), 2);
    Ok(())
}
