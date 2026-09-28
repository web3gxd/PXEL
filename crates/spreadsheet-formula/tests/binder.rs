use spreadsheet_domain::{
    formula::{AxisReferenceMode, Node, ReferenceError},
    workbook::*,
};
use spreadsheet_formula::binder::bind;

fn workbook(reverse: bool) -> Result<(Workbook, CellAddress), Box<dyn std::error::Error>> {
    let document = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let mut rows = vec![
        RowId::from_bytes([3; 16])?,
        RowId::from_bytes([4; 16])?,
        RowId::from_bytes([5; 16])?,
    ];
    if reverse {
        rows.swap(0, 1);
    }
    let columns = vec![
        ColumnId::from_bytes([6; 16])?,
        ColumnId::from_bytes([7; 16])?,
        ColumnId::from_bytes([8; 16])?,
    ];
    let origin = CellAddress {
        sheet,
        row: rows[2],
        column: columns[2],
    };
    let book = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet1".into(),
                rows,
                columns,
            },
        },
        DocumentVersion::new(1)?,
    )?;
    Ok((book, origin))
}

#[test]
fn binding_uses_supplied_axis_projection_and_preserves_copy_modes()
-> Result<(), Box<dyn std::error::Error>> {
    let (book, origin) = workbook(false)?;
    let formula = bind("=A1+$A$1+A$1+$A1", &book, origin)?;
    let references: Vec<_> = formula
        .ast()
        .nodes()
        .iter()
        .filter_map(|node| {
            if let Node::Cell(reference) = node {
                Some(reference)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(references.len(), 4);
    for reference in &references {
        assert_eq!(reference.target_row, RowId::from_bytes([3; 16])?);
        assert_eq!(reference.target_column, ColumnId::from_bytes([6; 16])?);
    }
    assert_eq!(
        references[0].row_mode,
        AxisReferenceMode::Relative { original_delta: -2 }
    );
    assert_eq!(
        references[0].column_mode,
        AxisReferenceMode::Relative { original_delta: -2 }
    );
    assert_eq!(references[1].row_mode, AxisReferenceMode::Absolute);
    assert_eq!(references[1].column_mode, AxisReferenceMode::Absolute);
    assert_eq!(references[2].row_mode, AxisReferenceMode::Absolute);
    assert_eq!(references[3].column_mode, AxisReferenceMode::Absolute);
    let (reordered, origin) = workbook(true)?;
    let newer = bind("=A1", &reordered, origin)?;
    assert!(
        matches!(&newer.ast().nodes()[0],Node::Cell(reference) if reference.target_row==RowId::from_bytes([4;16])?)
    );
    assert_eq!(formula.bound_at_version(), book.version());
    Ok(())
}

#[test]
fn unallocated_or_unknown_references_are_explicit_and_origin_must_be_live()
-> Result<(), Box<dyn std::error::Error>> {
    let (book, origin) = workbook(false)?;
    let formula = bind("=A4", &book, origin)?;
    assert_eq!(
        formula.ast().nodes(),
        &[Node::InvalidReference(ReferenceError::OutOfBounds)]
    );
    let formula = bind("='missing sheet'!A1", &book, origin)?;
    assert_eq!(
        formula.ast().nodes(),
        &[Node::InvalidReference(ReferenceError::MissingSheet)]
    );
    let formula = bind("=sHeEt1!A1", &book, origin)?;
    assert!(matches!(formula.ast().nodes()[0], Node::Cell(_)));
    assert!(
        bind(
            "=1",
            &book,
            CellAddress {
                row: RowId::from_bytes([99; 16])?,
                ..origin
            }
        )
        .is_err()
    );
    Ok(())
}
