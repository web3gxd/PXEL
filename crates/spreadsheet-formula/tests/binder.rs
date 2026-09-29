use spreadsheet_domain::{
    formula::{AxisReferenceMode, Node, ReferenceError},
    workbook::*,
};
use spreadsheet_formula::binder::bind;
use spreadsheet_formula::{bound::render_current, canonical, parse};

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

#[test]
fn current_a1_text_follows_stable_targets_and_expands_range()
-> Result<(), Box<dyn std::error::Error>> {
    let (book, origin) = workbook(false)?;
    let formula = bind("=A1+$A$1+SUM(A1:A2)", &book, origin)?;
    assert_eq!(render_current(&formula, &book)?, formula.canonical());
    let sheet = book.sheets().first().ok_or("missing sheet")?;
    let inserted = RowId::from_bytes([9; 16])?;
    let shifted = Workbook::hydrate(
        book.document_id(),
        DocumentVersion::new(2)?,
        SheetSeed {
            id: sheet.id(),
            name: sheet.name().to_owned(),
            rows: vec![sheet.rows()[0], inserted, sheet.rows()[1], sheet.rows()[2]],
            columns: sheet.columns().to_vec(),
        },
        [(origin, CellContent::Formula(formula.clone()))],
    )?;
    let current = render_current(&formula, &shifted)?;
    assert_eq!(current, canonical(&parse("=A1+$A$1+SUM(A1:A3)")?.ast)?);
    let rebound = bind(&current, &shifted, origin)?;
    let old_targets: Vec<_> = formula
        .ast()
        .nodes()
        .iter()
        .filter_map(|node| match node {
            Node::Cell(reference) => Some((reference.target_row, reference.target_column)),
            _ => None,
        })
        .collect();
    let new_targets: Vec<_> = rebound
        .ast()
        .nodes()
        .iter()
        .filter_map(|node| match node {
            Node::Cell(reference) => Some((reference.target_row, reference.target_column)),
            _ => None,
        })
        .collect();
    assert_eq!(new_targets, old_targets);
    let old_range = formula.ast().nodes().iter().find_map(|node| match node {
        Node::Range(reference) => Some((reference.start.target_row, reference.end.target_row)),
        _ => None,
    });
    let new_range = rebound.ast().nodes().iter().find_map(|node| match node {
        Node::Range(reference) => Some((reference.start.target_row, reference.end.target_row)),
        _ => None,
    });
    assert_eq!(new_range, old_range);
    assert!(matches!(&rebound.ast().nodes()[0], Node::Cell(reference)
        if reference.row_mode == AxisReferenceMode::Relative { original_delta: -3 }));
    Ok(())
}

#[test]
fn deleted_rows_render_shrunken_ranges_and_typed_ref_errors()
-> Result<(), Box<dyn std::error::Error>> {
    let (book, origin) = workbook(false)?;
    let formula = bind("=SUM(A1:A2)", &book, origin)?;
    let book = book.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula,
        },
        DocumentVersion::new(2)?,
    )?;
    let first = book.sheets()[0].rows()[0];
    let middle = book.sheets()[0].rows()[1];
    let shortened = book.candidate(
        &Operation::DeleteRows {
            sheet: origin.sheet,
            rows: vec![first],
        },
        DocumentVersion::new(3)?,
    )?;
    let CellContent::Formula(formula) = shortened.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or("missing formula")?
    else {
        return Err("expected formula".into());
    };
    assert_eq!(render_current(formula, &shortened)?, "=SUM(A1:A1)");
    let invalid = shortened.candidate(
        &Operation::DeleteRows {
            sheet: origin.sheet,
            rows: vec![middle],
        },
        DocumentVersion::new(4)?,
    )?;
    let CellContent::Formula(formula) = invalid.sheets()[0]
        .cells()
        .get(&origin)
        .ok_or("missing formula")?
    else {
        return Err("expected formula".into());
    };
    assert_eq!(render_current(formula, &invalid)?, "=SUM(#REF!)");
    Ok(())
}
