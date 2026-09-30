use spreadsheet_domain::{style::*, workbook::*};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn book() -> Result<(Workbook, CellAddress), DomainError> {
    let address = CellAddress {
        sheet: SheetId::from_bytes([2; 16])?,
        row: RowId::from_bytes([3; 16])?,
        column: ColumnId::from_bytes([4; 16])?,
    };
    Ok((
        Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: address.sheet,
                    name: "Sheet1".into(),
                    rows: vec![address.row],
                    columns: vec![address.column],
                },
            },
            DocumentVersion::new(1)?,
        )?,
        address,
    ))
}
#[test]
fn mixed_preview_retains_version_and_rejects_duplicate_style_addresses_atomically() -> TestResult {
    let (book, a) = book()?;
    let styles = vec![(a, vec![StyleChange::FontBold(Change::Set(true))])];
    let candidate = book.preview_authored_candidate(
        &[Operation::SetCellValue {
            address: a,
            value: CellValue::Number(3.0),
        }],
        &styles,
        book.version(),
    )?;
    assert_eq!(candidate.version(), book.version());
    assert!(candidate.sheets()[0].styles()[&a].font_bold());
    assert!(candidate.cell_content(a)?.is_some());
    assert!(
        book.preview_authored_candidate(
            &[],
            &[styles[0].clone(), styles[0].clone()],
            book.version()
        )
        .is_err()
    );
    assert!(book.sheets()[0].styles().is_empty());
    assert!(book.cell_content(a)?.is_none());
    Ok(())
}
#[test]
fn snapshot_hydration_refuses_future_calculation_epoch() -> TestResult {
    let (book, _) = book()?;
    let mut image = book.snapshot_image()?;
    image.calculation_context = Some(CalculationContext::new(
        CalculationContextId::from_bytes([5; 16])?,
        2,
        86_400_000,
        0,
        1,
        [0; 32],
        "engine".into(),
        "profile".into(),
        CalculationReason::Explicit,
    )?);
    assert!(matches!(
        Workbook::hydrate_snapshot(image),
        Err(DomainError::InvalidCalculationContext)
    ));
    Ok(())
}
#[test]
fn mixed_preview_releases_content_and_style_capacity_before_final_admission() -> TestResult {
    let (_, a) = book()?;
    let rows = (1u128..=50_001)
        .map(|n| RowId::from_bytes(n.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let second = ColumnId::from_bytes([6; 16])?;
    let seed = SheetSeed {
        id: a.sheet,
        name: "Sheet1".into(),
        rows: rows.clone(),
        columns: vec![a.column, second],
    };
    let cells = rows.iter().take(50_000).flat_map(|row| {
        [a.column, second].map(|column| {
            (
                CellAddress {
                    sheet: a.sheet,
                    row: *row,
                    column,
                },
                CellContent::Value(CellValue::Number(1.0)),
            )
        })
    });
    let book = Workbook::hydrate(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        seed,
        cells,
    )?;
    let old = CellAddress { row: rows[0], ..a };
    let empty = CellAddress {
        row: rows[50_000],
        ..a
    };
    let another = CellAddress {
        column: second,
        ..empty
    };
    let first = book.preview_authored_candidate(
        &[Operation::ClearCell { address: old }],
        &[(empty, vec![StyleChange::FontBold(Change::Set(true))])],
        book.version(),
    )?;
    let next = first.preview_authored_candidate(
        &[Operation::SetCellValue {
            address: another,
            value: CellValue::Number(2.0),
        }],
        &[(empty, vec![StyleChange::FontBold(Change::Reset)])],
        book.version(),
    )?;
    assert_eq!(next.sheets()[0].cells().len(), MAX_CELLS);
    assert!(next.sheets()[0].styles().is_empty());
    assert!(
        first
            .preview_authored_candidate(
                &[Operation::SetCellValue {
                    address: another,
                    value: CellValue::Number(2.0)
                }],
                &[],
                book.version()
            )
            .is_err()
    );
    assert!(first.sheets()[0].styles().contains_key(&empty));
    Ok(())
}
