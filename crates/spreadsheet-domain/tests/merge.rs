use spreadsheet_domain::{
    MergeChange, MergeRange,
    style::{Change, StyleChange},
    workbook::{
        CellAddress, CellValue, ColumnId, DocumentId, DocumentVersion, DomainError, Operation,
        OperationId, RowId, SheetId, SheetSeed, Workbook,
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn populated_merge_clears_and_undo_restores_content_and_styles() -> TestResult {
    let (book, range, anchor, hidden) = fixture()?;
    let book = book.candidate(
        &Operation::SetCellValue {
            address: anchor,
            value: CellValue::Text("title".into()),
        },
        DocumentVersion::new(2)?,
    )?;
    let book = book.candidate(
        &Operation::SetCellValue {
            address: hidden,
            value: CellValue::Number(7.0),
        },
        DocumentVersion::new(3)?,
    )?;
    let styled_blank = CellAddress {
        column: hidden.column,
        ..anchor
    };
    let style = spreadsheet_domain::style_write::prepare_style_write(
        &book,
        &[hidden, styled_blank],
        &[StyleChange::FontBold(Change::Set(true))],
        spreadsheet_domain::style_write::StyleWriteKind::Range,
    )?;
    let book = book.candidate(&style, DocumentVersion::new(4)?)?;
    let original_content = book.cell_content(hidden)?.cloned();
    let original_style = book.sheets()[0].styles().get(&hidden).cloned();
    let operation = spreadsheet_domain::merge::prepare_merge_cells(&book, anchor.sheet, range)?;
    let merged = book.candidate(&operation, DocumentVersion::new(5)?)?;
    assert_eq!(merged.cell_content(anchor)?, book.cell_content(anchor)?);
    assert!(merged.cell_content(hidden)?.is_none());
    assert!(!merged.sheets()[0].styles().contains_key(&hidden));
    assert!(!merged.sheets()[0].styles().contains_key(&styled_blank));
    assert_eq!(
        Workbook::restore_snapshot(merged.snapshot_image()?, merged.hash())?.hash(),
        merged.hash()
    );
    let inverse = spreadsheet_domain::merge::inverse_merge_cells(
        &operation,
        OperationId::from_bytes([9; 16])?,
        DocumentVersion::new(5)?,
    )?;
    let restored = merged.candidate(&inverse, DocumentVersion::new(6)?)?;
    assert_eq!(restored.cell_content(hidden)?.cloned(), original_content);
    assert_eq!(
        restored.sheets()[0].styles().get(&hidden).cloned(),
        original_style
    );
    assert_eq!(
        restored.sheets()[0].styles().get(&styled_blank),
        book.sheets()[0].styles().get(&styled_blank)
    );
    let redone = spreadsheet_domain::merge::inverse_merge_cells(
        &inverse,
        OperationId::from_bytes([10; 16])?,
        DocumentVersion::new(6)?,
    )?;
    let redone = restored.candidate(&redone, DocumentVersion::new(7)?)?;
    assert!(redone.cell_content(hidden)?.is_none());
    assert!(redone.sheets()[0].merges().contains(&range));
    let unmerged = merged.candidate(
        &merge_operation(range, false, true)?,
        DocumentVersion::new(6)?,
    )?;
    assert!(unmerged.cell_content(hidden)?.is_none());
    assert!(!unmerged.sheets()[0].styles().contains_key(&hidden));
    Ok(())
}

#[test]
fn populated_merge_rejects_stale_preimages_without_clearing_current_data() -> TestResult {
    let (book, range, anchor, hidden) = fixture()?;
    let old = book.candidate(
        &Operation::SetCellValue {
            address: hidden,
            value: CellValue::Number(7.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let prepared = spreadsheet_domain::merge::prepare_merge_cells(&old, anchor.sheet, range)?;
    let current = old.candidate(
        &Operation::SetCellValue {
            address: hidden,
            value: CellValue::Number(9.0),
        },
        DocumentVersion::new(3)?,
    )?;
    let before = current.hash();
    assert!(matches!(
        current.candidate(&prepared, DocumentVersion::new(4)?),
        Err(DomainError::PreimageConflict)
    ));
    assert_eq!(current.hash(), before);
    assert!(current.sheets()[0].merges().is_empty());
    Ok(())
}

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
