use spreadsheet_domain::{
    MergeChange, MergeRange,
    style::{Change, StyleChange},
    workbook::{
        CellAddress, CellValue, ColumnId, DocumentId, DocumentVersion, DomainError, Operation,
        OperationId, RowId, SheetId, SheetSeed, Workbook,
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture() -> Result<(Workbook, MergeRange, CellAddress, CellAddress), DomainError> {
    let sheet = SheetId::from_bytes([2; 16])?;
    let rows = [RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?];
    let columns = [
        ColumnId::from_bytes([5; 16])?,
        ColumnId::from_bytes([6; 16])?,
    ];
    let address = |row, column| CellAddress { sheet, row, column };
    let book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet".into(),
                rows: rows.to_vec(),
                columns: columns.to_vec(),
            },
        },
        DocumentVersion::new(1)?,
    )?;
    Ok((
        book,
        MergeRange::new(rows[0], rows[1], columns[0], columns[1]),
        address(rows[0], columns[0]),
        address(rows[1], columns[1]),
    ))
}

fn merge_operation(
    range: MergeRange,
    merged: bool,
    previous: bool,
) -> Result<Operation, DomainError> {
    Ok(Operation::MergeWrite {
        sheet: SheetId::from_bytes([2; 16])?,
        change: MergeChange {
            range,
            merged,
            previous,
        },
        undoes: None,
        undoes_version: None,
    })
}

#[test]
fn merge_guards_hidden_cells_and_unmerge_restores_only_membership() -> TestResult {
    let (book, range, anchor, hidden) = fixture()?;
    let operation = merge_operation(range, true, false)?;
    let merged = book.candidate(&operation, DocumentVersion::new(2)?)?;
    assert!(merged.sheets()[0].merges().contains(&range));

    let changed_anchor = merged.candidate(
        &Operation::SetCellValue {
            address: anchor,
            value: CellValue::Text("visible".into()),
        },
        DocumentVersion::new(3)?,
    )?;
    assert!(matches!(
        changed_anchor.candidate(
            &Operation::SetCellValue {
                address: hidden,
                value: CellValue::Text("hidden".into()),
            },
            DocumentVersion::new(4)?,
        ),
        Err(DomainError::MergeConflict)
    ));
    assert!(matches!(
        spreadsheet_domain::style_write::prepare_style_write(
            &changed_anchor,
            &[hidden],
            &[StyleChange::FontBold(Change::Set(true))],
            spreadsheet_domain::style_write::StyleWriteKind::Cell,
        ),
        Err(DomainError::MergeConflict)
    ));

    let unmerge = Operation::MergeWrite {
        sheet: anchor.sheet,
        change: MergeChange {
            range,
            merged: false,
            previous: true,
        },
        undoes: Some(OperationId::from_bytes([9; 16])?),
        undoes_version: Some(DocumentVersion::new(2)?),
    };
    let unmerged = changed_anchor.candidate(&unmerge, DocumentVersion::new(4)?)?;
    assert!(!unmerged.sheets()[0].merges().contains(&range));
    assert!(unmerged.cell_content(anchor)?.is_some());
    assert_eq!(
        Workbook::restore_snapshot(unmerged.snapshot_image()?, unmerged.hash())?.hash(),
        unmerged.hash()
    );
    Ok(())
}

#[test]
fn merge_rejects_hidden_content_and_overlapping_ranges() -> TestResult {
    let (book, range, _anchor, hidden) = fixture()?;
    let authored = book.candidate(
        &Operation::SetCellValue {
            address: hidden,
            value: CellValue::Number(7.0),
        },
        DocumentVersion::new(2)?,
    )?;
    assert!(matches!(
        authored.candidate(
            &merge_operation(range, true, false)?,
            DocumentVersion::new(3)?
        ),
        Err(DomainError::MergeConflict)
    ));
    let merged = book.candidate(
        &merge_operation(range, true, false)?,
        DocumentVersion::new(2)?,
    )?;
    let overlapping = MergeRange::new(
        range.first_row(),
        range.last_row(),
        range.first_column(),
        range.first_column(),
    );
    assert!(matches!(
        merged.candidate(
            &merge_operation(overlapping, true, false)?,
            DocumentVersion::new(3)?,
        ),
        Err(DomainError::MergeConflict)
    ));
    Ok(())
}
