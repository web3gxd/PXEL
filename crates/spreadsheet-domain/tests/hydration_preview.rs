use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};
use spreadsheet_domain::workbook::*;

fn seed() -> Result<(DocumentId, SheetSeed, CellAddress), DomainError> {
    let address = CellAddress {
        sheet: SheetId::from_bytes([2; 16])?,
        row: RowId::from_bytes([3; 16])?,
        column: ColumnId::from_bytes([4; 16])?,
    };
    Ok((
        DocumentId::from_bytes([1; 16])?,
        SheetSeed {
            id: address.sheet,
            name: "Sheet".into(),
            rows: vec![address.row],
            columns: vec![address.column],
        },
        address,
    ))
}

#[test]
fn hydration_matches_canonical_replay_and_rejects_duplicate_blank() -> Result<(), DomainError> {
    let (id, sheet, address) = seed()?;
    let original = Workbook::empty(id)
        .candidate(
            &Operation::CreateWorkbook {
                sheet: sheet.clone(),
            },
            DocumentVersion::new(1)?,
        )?
        .candidate(
            &Operation::SetCellValue {
                address,
                value: CellValue::Number(-0.0),
            },
            DocumentVersion::new(2)?,
        )?;
    let hydrated = Workbook::hydrate(
        id,
        original.version(),
        sheet.clone(),
        [(address, CellValue::Number(0.0).into())],
    )?;
    assert_eq!(hydrated.hash(), original.hash());
    assert!(matches!(
        Workbook::hydrate(
            id,
            original.version(),
            sheet,
            [
                (address, CellValue::Blank.into()),
                (address, CellValue::Number(1.0).into())
            ]
        ),
        Err(DomainError::InvalidAxis)
    ));
    Ok(())
}

#[test]
fn preview_is_atomic_and_does_not_relax_canonical_version_checks() -> Result<(), DomainError> {
    let (id, sheet, address) = seed()?;
    let book = Workbook::hydrate(id, DocumentVersion::new(1)?, sheet, [])?;
    let initial = book.hash();
    let edit = Operation::SetCellValue {
        address,
        value: CellValue::Number(7.0),
    };
    let preview = book.preview_candidate(std::slice::from_ref(&edit), book.version())?;
    assert_eq!(preview.version(), book.version());
    assert_ne!(preview.hash(), initial);
    assert!(matches!(
        book.candidate(&edit, book.version()),
        Err(DomainError::VersionConflict)
    ));
    assert!(matches!(
        book.preview_candidate(
            &[edit.clone(), Operation::ClearCell { address }],
            book.version()
        ),
        Err(DomainError::InvalidAxis)
    ));
    let missing = CellAddress {
        row: RowId::from_bytes([9; 16])?,
        ..address
    };
    assert!(matches!(
        book.preview_candidate(
            &[edit.clone(), Operation::ClearCell { address: missing }],
            book.version()
        ),
        Err(DomainError::MissingRow)
    ));
    assert_eq!(book.hash(), initial);
    let future = book.preview_candidate(&[edit], DocumentVersion::new(5)?)?;
    assert_eq!(future.version().get(), 5);
    assert!(matches!(
        future.preview_candidate(&[], book.version()),
        Err(DomainError::VersionConflict)
    ));
    Ok(())
}

#[test]
fn formula_hydration_and_coalesced_preview_preserve_metadata_hashes()
-> Result<(), Box<dyn std::error::Error>> {
    let (id, sheet, address) = seed()?;
    let book = Workbook::hydrate(id, DocumentVersion::new(1)?, sheet.clone(), [])?;
    let formula = Formula::new(
        "=42".into(),
        "=42".into(),
        BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(42.0)])?,
        address,
        DocumentVersion::new(9)?,
    )?;
    let operation = Operation::SetCellFormula {
        address,
        formula: formula.clone(),
    };
    assert!(matches!(
        book.preview_candidate(std::slice::from_ref(&operation), book.version()),
        Err(DomainError::InvalidFormula)
    ));
    let preview = book.preview_candidate(&[operation], DocumentVersion::new(10)?)?;
    let hydrated = Workbook::hydrate(
        id,
        preview.version(),
        sheet.clone(),
        [(address, CellContent::Formula(formula.clone()))],
    )?;
    assert_eq!(hydrated.hash(), preview.hash());
    assert_eq!(hydrated.text_bytes(), formula.text_bytes());
    assert_eq!(hydrated.ast_memory_bytes(), formula.ast_memory_bytes());
    let wrong_address = CellAddress {
        row: RowId::from_bytes([8; 16])?,
        ..address
    };
    assert!(matches!(
        Workbook::hydrate(
            id,
            preview.version(),
            sheet.clone(),
            [(wrong_address, CellContent::Formula(formula.clone()))]
        ),
        Err(DomainError::InvalidFormula)
    ));
    assert!(matches!(
        Workbook::hydrate(
            id,
            DocumentVersion::new(8)?,
            sheet,
            [(address, CellContent::Formula(formula))]
        ),
        Err(DomainError::InvalidFormula)
    ));
    Ok(())
}

#[test]
fn preview_budget_failure_rolls_back_and_clear_releases_capacity() -> Result<(), DomainError> {
    let (id, mut sheet, address) = seed()?;
    sheet.rows = (1_u128..=514)
        .map(|n| RowId::from_bytes(n.to_be_bytes()))
        .collect::<Result<_, _>>()?;
    let addresses: Vec<_> = sheet
        .rows
        .iter()
        .map(|row| CellAddress {
            row: *row,
            ..address
        })
        .collect();
    let cells = addresses[..512]
        .iter()
        .map(|address| (*address, CellValue::Text("x".repeat(MAX_TEXT_BYTES)).into()));
    let book = Workbook::hydrate(id, DocumentVersion::new(1)?, sheet.clone(), cells)?;
    assert_eq!(book.text_bytes(), MAX_WORKBOOK_TEXT_BYTES);
    let hash = book.hash();
    let oversized = [
        Operation::SetCellValue {
            address: addresses[512],
            value: CellValue::Number(3.0),
        },
        Operation::SetCellValue {
            address: addresses[513],
            value: CellValue::Text("x".into()),
        },
    ];
    assert!(matches!(
        book.preview_candidate(&oversized, book.version()),
        Err(DomainError::LimitExceeded)
    ));
    assert_eq!(book.hash(), hash);
    assert_eq!(book.text_bytes(), MAX_WORKBOOK_TEXT_BYTES);
    let released = book.preview_candidate(
        &[
            Operation::ClearCell {
                address: addresses[0],
            },
            Operation::SetCellValue {
                address: addresses[513],
                value: CellValue::Text("x".repeat(MAX_TEXT_BYTES)),
            },
        ],
        book.version(),
    )?;
    assert_eq!(released.text_bytes(), MAX_WORKBOOK_TEXT_BYTES);
    let excessive = addresses[..513]
        .iter()
        .map(|address| (*address, CellValue::Text("x".repeat(MAX_TEXT_BYTES)).into()));
    assert!(matches!(
        Workbook::hydrate(id, book.version(), sheet, excessive),
        Err(DomainError::LimitExceeded)
    ));
    Ok(())
}

#[test]
fn hydration_rejects_invalid_axes_values_versions_and_unbounded_stream() -> Result<(), DomainError>
{
    let (id, sheet, address) = seed()?;
    assert!(matches!(
        Workbook::hydrate(id, DocumentVersion::ZERO, sheet.clone(), []),
        Err(DomainError::VersionConflict)
    ));
    let mut duplicated = sheet.clone();
    duplicated.rows.push(address.row);
    assert!(matches!(
        Workbook::hydrate(id, DocumentVersion::new(1)?, duplicated, []),
        Err(DomainError::InvalidAxis)
    ));
    assert!(matches!(
        Workbook::hydrate(
            id,
            DocumentVersion::new(1)?,
            sheet.clone(),
            [(address, CellValue::Number(f64::NAN).into())]
        ),
        Err(DomainError::InvalidValue)
    ));
    let mut large = sheet;
    large.rows = (1_u128..=MAX_ROWS as u128)
        .map(|n| RowId::from_bytes(n.to_be_bytes()))
        .collect::<Result<_, _>>()?;
    large.columns.push(ColumnId::from_bytes([7; 16])?);
    let cells = large
        .rows
        .clone()
        .into_iter()
        .map(|row| (CellAddress { row, ..address }, CellValue::Blank.into()))
        .chain(std::iter::once((
            CellAddress {
                column: large.columns[1],
                row: large.rows[0],
                ..address
            },
            CellValue::Blank.into(),
        )));
    assert!(matches!(
        Workbook::hydrate(id, DocumentVersion::new(1)?, large, cells),
        Err(DomainError::LimitExceeded)
    ));
    Ok(())
}
