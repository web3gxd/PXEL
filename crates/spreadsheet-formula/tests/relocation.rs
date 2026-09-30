use spreadsheet_domain::workbook::*;
use spreadsheet_formula::{
    binder::bind,
    bound::render_current,
    relocation::{MoveRequest, plan_move},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture() -> Result<Workbook, DomainError> {
    Workbook::hydrate_sheets(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        [SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet".into(),
            rows: (1u128..=4)
                .map(|n| RowId::from_bytes(n.to_be_bytes()))
                .collect::<Result<_, _>>()?,
            columns: (10u128..=15)
                .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
                .collect::<Result<_, _>>()?,
        }],
        [],
    )
}
fn at(book: &Workbook, row: usize, column: usize) -> CellAddress {
    let sheet = &book.sheets()[0];
    CellAddress {
        sheet: sheet.id(),
        row: sheet.rows()[row],
        column: sheet.columns()[column],
    }
}
fn set(
    book: Workbook,
    address: CellAddress,
    formula: &str,
) -> Result<Workbook, Box<dyn std::error::Error>> {
    let bound = bind(formula, &book, address)?;
    let next = DocumentVersion::new(book.version().get() + 1)?;
    Ok(book.candidate(
        &Operation::SetCellFormula {
            address,
            formula: bound,
        },
        next,
    )?)
}

#[test]
fn moving_formula_preserves_targets_and_recomputes_copy_offsets() -> TestResult {
    let book = fixture()?;
    let source = at(&book, 0, 1);
    let destination = at(&book, 2, 3);
    let book = set(book, source, "=A1+$A1+A$1+$A$1")?;
    let hash = book.hash();
    let plan = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    )?;
    assert_eq!(plan.input_version, book.version());
    assert_eq!(plan.input_hash, hash);
    let written = plan
        .writes
        .iter()
        .find(|w| w.address == destination)
        .ok_or("missing destination")?;
    let Some(CellContent::Formula(formula)) = &written.content else {
        return Err("missing formula".into());
    };
    assert_eq!(formula.origin(), destination);
    assert_eq!(render_current(formula, &book)?, "=(((A1+$A1)+A$1)+$A$1)");
    assert_eq!(formula.source(), "=A1+$A1+A$1+$A$1");
    use spreadsheet_domain::formula::{
        AxisReferenceMode::{Absolute, Relative},
        Node,
    };
    let modes = formula
        .ast()
        .nodes()
        .iter()
        .filter_map(|node| match node {
            Node::Cell(reference) => Some((reference.row_mode, reference.column_mode)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        modes,
        vec![
            (
                Relative { original_delta: -2 },
                Relative { original_delta: -3 }
            ),
            (Relative { original_delta: -2 }, Absolute),
            (Absolute, Relative { original_delta: -3 }),
            (Absolute, Absolute),
        ]
    );
    let copied =
        spreadsheet_formula::copy_formula_text(&render_current(formula, &book)?, (2, 3), (3, 4))?;
    assert_eq!(copied, "=(((B2+$A2)+B$1)+$A$1)");
    assert_eq!(book.hash(), hash);
    Ok(())
}

#[test]
fn overlapping_move_rewrites_inbound_references_once_and_captures_preimages() -> TestResult {
    let mut book = fixture()?;
    let a = at(&book, 0, 0);
    let b = at(&book, 0, 1);
    let c = at(&book, 0, 2);
    let external = at(&book, 1, 0);
    book = book.candidate(
        &Operation::SetCellValue {
            address: a,
            value: CellValue::Number(7.0),
        },
        DocumentVersion::new(2)?,
    )?;
    book = set(book, b, "=A1")?;
    book = set(book, external, "=$A$1+$B$1")?;
    let hash = book.hash();
    let plan = plan_move(
        &book,
        MoveRequest {
            source: a,
            destination: b,
            rows: 1,
            columns: 2,
        },
    )?;
    assert_eq!(plan.writes.len(), 4);
    assert!(
        plan.writes
            .iter()
            .any(|w| w.address == a && w.content.is_none())
    );
    assert!(plan.writes.iter().any(|w| w.address == b
        && w.content == Some(CellContent::Value(CellValue::Number(7.0)))
        && matches!(w.previous, Some(CellContent::Formula(_)))));
    for (address, text) in [(c, "=B1"), (external, "=($B$1+$C$1)")] {
        let write = plan
            .writes
            .iter()
            .find(|w| w.address == address)
            .ok_or("missing rewritten formula")?;
        let Some(CellContent::Formula(formula)) = &write.content else {
            return Err("missing formula".into());
        };
        assert_eq!(render_current(formula, &book)?, text);
    }
    assert_eq!(book.hash(), hash);
    Ok(())
}

#[test]
fn a_partial_range_reference_rejects_the_whole_plan() -> TestResult {
    let book = fixture()?;
    let source = at(&book, 0, 0);
    let destination = at(&book, 2, 0);
    let external = at(&book, 1, 3);
    let book = set(book, external, "=SUM(A1:C1)")?;
    let hash = book.hash();
    assert!(matches!(
        plan_move(
            &book,
            MoveRequest {
                source,
                destination,
                rows: 1,
                columns: 1
            }
        ),
        Err(spreadsheet_formula::relocation::MoveError::PartialRange)
    ));
    assert_eq!(book.hash(), hash);
    Ok(())
}

fn preview(book: &Workbook, writes: &[GuardedContentWrite]) -> Result<Workbook, DomainError> {
    let operations = writes
        .iter()
        .map(|write| match &write.content {
            None => Operation::ClearCell {
                address: write.address,
            },
            Some(CellContent::Value(value)) => Operation::SetCellValue {
                address: write.address,
                value: value.clone(),
            },
            Some(CellContent::Formula(formula)) => Operation::SetCellFormula {
                address: write.address,
                formula: formula.clone(),
            },
        })
        .collect::<Vec<_>>();
    book.preview_candidate(&operations, DocumentVersion::new(book.version().get() + 1)?)
}

#[test]
fn overwritten_formula_is_not_transformed_and_blank_targets_follow_cut() -> TestResult {
    let book = fixture()?;
    let source = at(&book, 0, 0);
    let destination = at(&book, 2, 0);
    let external = at(&book, 1, 3);
    // This partial range is about to be overwritten and must not block the move.
    let book = set(book, destination, "=SUM(A1:C1)")?;
    let book = set(book, external, "=$A$1")?;
    let plan = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    )?;
    let moved = preview(&book, &plan.writes)?;
    assert!(moved.cell_content(destination)?.is_none());
    assert!(moved.cell_content(source)?.is_none());
    let Some(CellContent::Formula(formula)) = moved.cell_content(external)? else {
        return Err("missing reference".into());
    };
    assert_eq!(render_current(formula, &moved)?, "=$A$3");
    assert_eq!(moved.sheets()[0].cells().len(), 1);
    Ok(())
}

#[test]
fn contained_reversed_range_keeps_endpoint_order_and_absolute_modes() -> TestResult {
    let book = fixture()?;
    let source = at(&book, 0, 0);
    let destination = at(&book, 2, 2);
    let external = at(&book, 1, 5);
    let book = set(book, external, "=SUM($B$1:A$1)")?;
    let plan = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 2,
        },
    )?;
    let moved = preview(&book, &plan.writes)?;
    let Some(CellContent::Formula(formula)) = moved.cell_content(external)? else {
        return Err("missing range".into());
    };
    assert_eq!(render_current(formula, &moved)?, "=SUM($D$3:C$3)");
    Ok(())
}

#[test]
fn cross_sheet_inbound_reference_follows_cut() -> TestResult {
    let original = fixture()?;
    let first = &original.sheets()[0];
    let second_id = SheetId::from_bytes([3; 16])?;
    let mut book = Workbook::hydrate_sheets(
        original.document_id(),
        original.version(),
        [
            SheetSeed {
                id: first.id(),
                name: first.name().into(),
                rows: first.rows().to_vec(),
                columns: first.columns().to_vec(),
            },
            SheetSeed {
                id: second_id,
                name: "Other".into(),
                rows: (101u128..=104)
                    .map(|n| RowId::from_bytes(n.to_be_bytes()))
                    .collect::<Result<_, _>>()?,
                columns: (110u128..=115)
                    .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
                    .collect::<Result<_, _>>()?,
            },
        ],
        [],
    )?;
    let source = at(&book, 0, 0);
    let destination = at(&book, 2, 2);
    let external = CellAddress {
        sheet: second_id,
        row: book.sheets()[1].rows()[0],
        column: book.sheets()[1].columns()[0],
    };
    book = set(book, external, "=Sheet!$A$1")?;
    let plan = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    )?;
    let moved = preview(&book, &plan.writes)?;
    let Some(CellContent::Formula(formula)) = moved.cell_content(external)? else {
        return Err("missing cross-sheet reference".into());
    };
    // Current bound formatter consistently quotes sheet names.
    assert_eq!(render_current(formula, &moved)?, "='Sheet'!$C$3");
    Ok(())
}

#[test]
fn invalid_geometry_is_rejected_without_mutation() -> TestResult {
    let book = fixture()?;
    let source = at(&book, 0, 0);
    let destination = at(&book, 3, 5);
    let hash = book.hash();
    for (rows, columns) in [(0, 1), (1, 0), (2, 1), (1, 2)] {
        assert!(matches!(
            plan_move(
                &book,
                MoveRequest {
                    source,
                    destination,
                    rows,
                    columns
                }
            ),
            Err(spreadsheet_formula::relocation::MoveError::InvalidRectangle)
        ));
    }
    assert!(matches!(
        plan_move(
            &book,
            MoveRequest {
                source,
                destination: source,
                rows: 1,
                columns: 1
            }
        ),
        Err(spreadsheet_formula::relocation::MoveError::NoChange)
    ));
    assert!(matches!(
        plan_move(
            &book,
            MoveRequest {
                source,
                destination,
                rows: 1001,
                columns: 1
            }
        ),
        Err(spreadsheet_formula::relocation::MoveError::LimitExceeded)
    ));
    assert_eq!(book.hash(), hash);
    Ok(())
}

#[test]
fn leftward_overlap_and_inverse_preimages_restore_content() -> TestResult {
    let book = fixture()?;
    let a = at(&book, 0, 0);
    let b = at(&book, 0, 1);
    let c = at(&book, 0, 2);
    let external = at(&book, 1, 0);
    let book = set(book, b, "=C1")?;
    let book = set(book, c, "=B1")?;
    let book = set(book, external, "=$B$1+$C$1")?;
    let plan = plan_move(
        &book,
        MoveRequest {
            source: b,
            destination: a,
            rows: 1,
            columns: 2,
        },
    )?;
    let moved = preview(&book, &plan.writes)?;
    for (address, text) in [(a, "=B1"), (b, "=A1"), (external, "=($A$1+$B$1)")] {
        let Some(CellContent::Formula(formula)) = moved.cell_content(address)? else {
            return Err("missing moved formula".into());
        };
        assert_eq!(render_current(formula, &moved)?, text);
    }
    assert!(moved.cell_content(c)?.is_none());
    let inverse = plan
        .writes
        .iter()
        .map(|write| GuardedContentWrite {
            address: write.address,
            content: write.previous.clone(),
            previous: write.content.clone(),
        })
        .collect::<Vec<_>>();
    let restored = preview(&moved, &inverse)?;
    assert_eq!(restored.sheets()[0].cells(), book.sheets()[0].cells());
    let again = plan_move(
        &book,
        MoveRequest {
            source: b,
            destination: a,
            rows: 1,
            columns: 2,
        },
    )?;
    assert_eq!(again.writes, plan.writes);
    Ok(())
}

#[test]
fn cancelled_scan_discards_the_entire_plan() -> TestResult {
    let book = fixture()?;
    let source = at(&book, 0, 0);
    let destination = at(&book, 2, 2);
    let external = at(&book, 1, 4);
    let book = set(book, external, "=A1")?;
    let second_external = at(&book, 1, 5);
    let book = set(book, second_external, "=A1")?;
    let hash = book.hash();
    let mut checkpoints = 0;
    let result = spreadsheet_formula::relocation::plan_move_cancellable(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
        || {
            checkpoints += 1;
            checkpoints >= 5
        },
    );
    assert!(matches!(
        result,
        Err(spreadsheet_formula::relocation::MoveError::Cancelled)
    ));
    assert_eq!(book.hash(), hash);
    Ok(())
}

#[test]
fn independent_formula_retains_exact_ast_and_original_binding_epoch() -> TestResult {
    let book = fixture()?;
    let external = at(&book, 1, 4);
    let book = set(book, external, "=42")?;
    let unrelated = at(&book, 1, 5);
    let book = set(book, unrelated, "=43")?;
    let source = at(&book, 0, 0);
    let destination = at(&book, 0, 1);
    let original = book.cell_content(external)?.cloned();
    let plan = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    )?;
    assert_eq!(plan.writes.len(), 2);
    assert!(!plan.writes.iter().any(|write| write.address == external));
    let moved = preview(&book, &plan.writes)?;
    assert_eq!(moved.cell_content(external)?.cloned(), original);
    Ok(())
}

#[test]
fn inbound_reference_footprint_is_bounded_independently_of_source_size() -> TestResult {
    let sheet = SheetId::from_bytes([2; 16])?;
    let seed = SheetSeed {
        id: sheet,
        name: "Sheet".into(),
        rows: (1u128..=10000)
            .map(|n| RowId::from_bytes(n.to_be_bytes()))
            .collect::<Result<_, _>>()?,
        columns: (20001u128..=20003)
            .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let empty = Workbook::hydrate_sheets(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        [seed.clone()],
        [],
    )?;
    let cells = seed
        .rows
        .iter()
        .map(|&row| {
            let address = CellAddress {
                sheet,
                row,
                column: seed.columns[2],
            };
            Ok((address, CellContent::Formula(bind("=A1", &empty, address)?)))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let book = Workbook::hydrate_sheets(
        empty.document_id(),
        DocumentVersion::new(2)?,
        [seed.clone()],
        cells,
    )?;
    let hash = book.hash();
    let request = MoveRequest {
        source: CellAddress {
            sheet,
            row: seed.rows[0],
            column: seed.columns[0],
        },
        destination: CellAddress {
            sheet,
            row: seed.rows[0],
            column: seed.columns[1],
        },
        rows: 1,
        columns: 1,
    };
    assert!(matches!(
        plan_move(&book, request),
        Err(spreadsheet_formula::relocation::MoveError::LimitExceeded)
    ));
    assert_eq!(book.hash(), hash);
    Ok(())
}

proptest::proptest! {
    #[test]
    fn simultaneous_value_moves_match_frozen_rectangle(
        source_column in 0usize..4, destination_column in 0usize..4,
        width in 1usize..=3, values in proptest::collection::vec(-1000i32..1000,6)
    ) {
        proptest::prop_assume!(source_column!=destination_column);
        proptest::prop_assume!(source_column+width<=6 && destination_column+width<=6);
        let result=(|| -> Result<(),Box<dyn std::error::Error>> {
            let mut book=fixture()?;
            for (column,value) in values.iter().enumerate() {
                book=book.candidate(&Operation::SetCellValue {
                    address:at(&book,0,column),value:CellValue::Number(f64::from(*value))
                },DocumentVersion::new(book.version().get()+1)?)?;
            }
            let hash=book.hash();
            let plan=plan_move(&book,MoveRequest {
                source:at(&book,0,source_column),destination:at(&book,0,destination_column),
                rows:1,columns:width as u32
            })?;
            let moved=preview(&book,&plan.writes)?;
            for column in 0..6 {
                let expected=if (destination_column..destination_column+width).contains(&column) {
                    Some(CellContent::Value(CellValue::Number(f64::from(values[source_column+column-destination_column]))))
                } else if (source_column..source_column+width).contains(&column) {None}
                else {Some(CellContent::Value(CellValue::Number(f64::from(values[column]))))};
                if moved.cell_content(at(&moved,0,column))?.cloned()!=expected {
                    return Err("simultaneous move differs from frozen rectangle".into());
                }
            }
            if book.hash()!=hash {return Err("input mutated".into());}
            Ok(())
        })();
        proptest::prop_assert!(result.is_ok(),"{}",result.err().map(|e|e.to_string()).unwrap_or_default());
    }
}
