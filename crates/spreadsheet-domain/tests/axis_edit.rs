use spreadsheet_domain::{axis_edit::*, formula::*, workbook::*};
type ResultTest = Result<(), Box<dyn std::error::Error>>;

#[test]
fn deleted_ids_and_insertion_anchors_keep_typed_errors_without_relaxing_duplicates() -> ResultTest {
    for columns in [false, true] {
        let base = fixture()?;
        let deleted = at(&base, 1, 1);
        let delete = if columns {
            Operation::DeleteColumns {
                sheet: deleted.sheet,
                columns: vec![deleted.column],
            }
        } else {
            Operation::DeleteRows {
                sheet: deleted.sheet,
                rows: vec![deleted.row],
            }
        };
        let operation = prepare_axis_edit(&base, &delete, DocumentVersion::new(2)?)?;
        let book = base.candidate(&operation, DocumentVersion::new(2)?)?;
        let insert = if columns {
            Operation::InsertColumnsRebased {
                sheet: deleted.sheet,
                before: deleted.column,
                columns: vec![ColumnId::from_bytes([99; 16])?],
            }
        } else {
            Operation::InsertRowsRebased {
                sheet: deleted.sheet,
                before: deleted.row,
                rows: vec![RowId::from_bytes([99; 16])?],
            }
        };
        let expected = if columns {
            DomainError::MissingColumn
        } else {
            DomainError::MissingRow
        };
        let hash = book.hash();
        for operation in [delete, insert] {
            assert_eq!(
                prepare_axis_edit(&book, &operation, DocumentVersion::new(3)?),
                Err(AxisEditError::Domain(expected))
            );
            assert_eq!(book.hash(), hash);
        }
        let duplicate = if columns {
            Operation::DeleteColumns {
                sheet: deleted.sheet,
                columns: vec![book.sheets()[0].columns()[0]; 2],
            }
        } else {
            Operation::DeleteRows {
                sheet: deleted.sheet,
                rows: vec![book.sheets()[0].rows()[0]; 2],
            }
        };
        assert_eq!(
            prepare_axis_edit(&book, &duplicate, DocumentVersion::new(3)?),
            Err(AxisEditError::Domain(DomainError::InvalidAxis))
        );
    }
    Ok(())
}
fn fixture() -> Result<Workbook, DomainError> {
    Workbook::hydrate(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet".into(),
            rows: (3..8)
                .map(|n| RowId::from_bytes([n; 16]))
                .collect::<Result<_, _>>()?,
            columns: (8..13)
                .map(|n| ColumnId::from_bytes([n; 16]))
                .collect::<Result<_, _>>()?,
        },
        [],
    )
}
fn at(book: &Workbook, r: usize, c: usize) -> CellAddress {
    CellAddress {
        sheet: book.sheets()[0].id(),
        row: book.sheets()[0].rows()[r],
        column: book.sheets()[0].columns()[c],
    }
}
fn event(book: &Workbook, op: Operation) -> Result<CommittedEvent, DomainError> {
    let version = DocumentVersion::new(book.version().get() + 1)?;
    let next = book.candidate(&op, version)?;
    Ok(CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([version.get() as u8 + 20; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([19; 16])?),
        version,
        operation: op,
        state_hash: next.hash(),
    })
}
fn reference(
    origin: CellAddress,
    target: CellAddress,
    delta: i64,
) -> Result<Formula, FormulaValidationError> {
    Formula::new(
        "=A1".into(),
        "=A1".into(),
        BoundAst::new(
            AST_SCHEMA_VERSION,
            NodeId(0),
            vec![Node::Cell(BoundCellReference {
                target_sheet: target.sheet,
                target_row: target.row,
                target_column: target.column,
                row_mode: AxisReferenceMode::Relative {
                    original_delta: delta,
                },
                column_mode: AxisReferenceMode::Absolute,
            })],
        )?,
        origin,
        DocumentVersion::ZERO,
    )
}
#[test]
fn noncontiguous_rows_and_columns_restore_lifetime_identity_and_snapshot_floors() -> ResultTest {
    for columns in [false, true] {
        let base = fixture()?;
        let address = at(&base, 4, 4);
        let mut book = base.candidate(
            &Operation::SetCellValue {
                address,
                value: CellValue::Number(17.),
            },
            DocumentVersion::new(2)?,
        )?;
        let old = book.hash();
        let sheet = &book.sheets()[0];
        let legacy = if columns {
            Operation::DeleteColumns {
                sheet: sheet.id(),
                columns: vec![sheet.columns()[1], sheet.columns()[4]],
            }
        } else {
            Operation::DeleteRows {
                sheet: sheet.id(),
                rows: vec![sheet.rows()[1], sheet.rows()[4]],
            }
        };
        let op = prepare_axis_edit(&book, &legacy, DocumentVersion::new(3)?)?;
        let target = event(&book, op)?;
        book.apply_event(&target)?;
        assert!(book.cell_content(address).is_err());
        let inverse = prepare_axis_inverse(&book, &target, DocumentVersion::new(4)?)?;
        let undo = event(&book, inverse)?;
        book.apply_event(&undo)?;
        assert_eq!(
            book.cell_content(address)?,
            Some(&CellValue::Number(17.).into())
        );
        assert_eq!(book.address_admission_floor(address)?.get(), 4);
        assert_ne!(book.hash(), old);
        let image = book.snapshot_image()?;
        assert_eq!(
            Workbook::restore_snapshot(image.clone(), book.hash())?.hash(),
            book.hash()
        );
        let mut forged = image;
        forged.row_activation_floors.clear();
        forged.column_activation_floors.clear();
        assert!(Workbook::restore_snapshot(forged, book.hash()).is_err());
        let redo = prepare_axis_inverse(&book, &undo, DocumentVersion::new(5)?)?;
        let redo_event = event(&book, redo)?;
        book.apply_event(&redo_event)?;
        let image = book.snapshot_image()?;
        assert_eq!(
            Workbook::restore_snapshot(image, book.hash())?.hash(),
            book.hash()
        );
        let legacy = if columns {
            Operation::InsertColumnsRebased {
                sheet: address.sheet,
                before: book.sheets()[0].columns()[0],
                columns: vec![address.column],
            }
        } else {
            Operation::InsertRowsRebased {
                sheet: address.sheet,
                before: book.sheets()[0].rows()[0],
                rows: vec![address.row],
            }
        };
        assert!(book.candidate(&legacy, DocumentVersion::new(6)?).is_err());
        let restored = prepare_axis_inverse(&book, &redo_event, DocumentVersion::new(6)?)?;
        book = book.candidate(&restored, DocumentVersion::new(6)?)?;
        assert_eq!(book.address_admission_floor(address)?.get(), 6);
    }
    Ok(())
}
#[test]
fn deletion_restores_ref_and_exact_authored_ast_and_keeps_independent_edit() -> ResultTest {
    let base = fixture()?;
    let target = at(&base, 1, 0);
    let origin = at(&base, 3, 0);
    let independent = at(&base, 4, 4);
    let formula = reference(origin, target, -2)?;
    let mut book = base.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula: formula.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let op = prepare_axis_edit(
        &book,
        &Operation::DeleteRows {
            sheet: target.sheet,
            rows: vec![target.row],
        },
        DocumentVersion::new(3)?,
    )?;
    let deleted = event(&book, op)?;
    book.apply_event(&deleted)?;
    let Some(CellContent::Formula(current)) = book.cell_content(origin)? else {
        return Err("missing formula".into());
    };
    assert!(matches!(
        current.ast().nodes()[0],
        Node::InvalidReference(_)
    ));
    book = book.candidate(
        &Operation::SetCellValue {
            address: independent,
            value: CellValue::Text("later".into()),
        },
        DocumentVersion::new(4)?,
    )?;
    let inverse = prepare_axis_inverse(&book, &deleted, DocumentVersion::new(5)?)?;
    book = book.candidate(&inverse, DocumentVersion::new(5)?)?;
    assert_eq!(
        book.cell_content(origin)?,
        Some(&CellContent::Formula(formula))
    );
    assert_eq!(
        book.cell_content(independent)?,
        Some(&CellValue::Text("later".into()).into())
    );
    Ok(())
}
#[test]
fn omitted_frozen_cells_duplicate_writes_and_forged_geometry_are_atomic() -> ResultTest {
    let base = fixture()?;
    let address = at(&base, 1, 0);
    let book = base.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(3.),
        },
        DocumentVersion::new(2)?,
    )?;
    let op = prepare_axis_edit(
        &book,
        &Operation::DeleteRows {
            sheet: address.sheet,
            rows: vec![address.row],
        },
        DocumentVersion::new(3)?,
    )?;
    let before = book.hash();
    for mutation in 0..5 {
        let mut bad = op.clone();
        if let Operation::AxisEdit { change } = &mut bad {
            match mutation {
                0 => change.writes.clear(),
                1 => change.writes.push(change.writes[0].clone()),
                2 => change.before_order_hash = [0; 32],
                3 => change.inverse_delta = change.delta.clone(),
                _ => change.writes[0].previous = None,
            }
        }
        assert!(book.candidate(&bad, DocumentVersion::new(3)?).is_err());
        assert_eq!(book.hash(), before);
    }
    Ok(())
}
#[test]
fn unchanged_formula_not_frozen_and_new_independent_formula_offsets_rebased_on_inverse()
-> ResultTest {
    let base = fixture()?;
    let origin = at(&base, 0, 0);
    let target = at(&base, 0, 1);
    let constant = reference(origin, target, 0)?;
    let mut book = base.candidate(
        &Operation::SetCellFormula {
            address: origin,
            formula: constant.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let inserted = RowId::from_bytes([99; 16])?;
    let op = prepare_axis_edit(
        &book,
        &Operation::InsertRowsRebased {
            sheet: origin.sheet,
            before: book.sheets()[0].rows()[2],
            rows: vec![inserted],
        },
        DocumentVersion::new(3)?,
    )?;
    let Operation::AxisEdit { change } = &op else {
        return Err("wrong operation".into());
    };
    assert!(change.writes.is_empty());
    let target_event = event(&book, op)?;
    book.apply_event(&target_event)?;
    let late_origin = at(&book, 5, 2);
    let late_target = at(&book, 0, 2);
    let late_formula = reference(late_origin, late_target, -5)?;
    book = book.candidate(
        &Operation::SetCellFormula {
            address: late_origin,
            formula: late_formula,
        },
        DocumentVersion::new(4)?,
    )?;
    let inverse = prepare_axis_inverse(&book, &target_event, DocumentVersion::new(5)?)?;
    book = book.candidate(&inverse, DocumentVersion::new(5)?)?;
    assert_eq!(
        book.cell_content(origin)?,
        Some(&CellContent::Formula(constant))
    );
    let Some(CellContent::Formula(late)) = book.cell_content(late_origin)? else {
        return Err("missing late".into());
    };
    let Node::Cell(r) = &late.ast().nodes()[0] else {
        return Err("missing reference".into());
    };
    assert_eq!(
        r.row_mode,
        AxisReferenceMode::Relative { original_delta: -4 }
    );
    Ok(())
}

#[test]
fn cross_sheet_ranges_restore_shrunk_reversed_and_fully_deleted_spans() -> ResultTest {
    for columns in [false, true] {
        for reverse in [false, true] {
            for full in [false, true] {
                let base = fixture()?;
                let original = base.snapshot_image()?.sheets.remove(0).sheet;
                let second = SheetSeed {
                    id: SheetId::from_bytes([40; 16])?,
                    name: "Other".into(),
                    rows: vec![RowId::from_bytes([41; 16])?],
                    columns: vec![ColumnId::from_bytes([42; 16])?],
                };
                let origin = CellAddress {
                    sheet: second.id,
                    row: second.rows[0],
                    column: second.columns[0],
                };
                let low = if full { 1 } else { 0 };
                let high = if full { 2 } else { 4 };
                let bound = |n: usize| BoundCellReference {
                    target_sheet: original.id,
                    target_row: original.rows[if columns { 0 } else { n }],
                    target_column: original.columns[if columns { n } else { 0 }],
                    row_mode: AxisReferenceMode::Absolute,
                    column_mode: AxisReferenceMode::Absolute,
                };
                let (a, b) = if reverse { (high, low) } else { (low, high) };
                let formula = Formula::new(
                    "=A1:A5".into(),
                    "=A1:A5".into(),
                    BoundAst::new(
                        AST_SCHEMA_VERSION,
                        NodeId(0),
                        vec![Node::Range(BoundRangeReference {
                            start: bound(a),
                            end: bound(b),
                        })],
                    )?,
                    origin,
                    DocumentVersion::ZERO,
                )?;
                let mut book = Workbook::hydrate_sheets(
                    base.document_id(),
                    base.version(),
                    [original.clone(), second],
                    [(origin, CellContent::Formula(formula.clone()))],
                )?;
                let indices = if full { vec![1, 2] } else { vec![0, 2, 4] };
                let legacy = if columns {
                    Operation::DeleteColumns {
                        sheet: original.id,
                        columns: indices.iter().map(|n| original.columns[*n]).collect(),
                    }
                } else {
                    Operation::DeleteRows {
                        sheet: original.id,
                        rows: indices.iter().map(|n| original.rows[*n]).collect(),
                    }
                };
                let operation = prepare_axis_edit(&book, &legacy, DocumentVersion::new(2)?)?;
                let target = event(&book, operation)?;
                book.apply_event(&target)?;
                let Some(CellContent::Formula(current)) = book.cell_content(origin)? else {
                    return Err("missing range".into());
                };
                if full {
                    assert!(matches!(
                        current.ast().nodes()[0],
                        Node::InvalidReference(_)
                    ));
                } else {
                    let Node::Range(range) = &current.ast().nodes()[0] else {
                        return Err("expected shrunk range".into());
                    };
                    assert_eq!(range.start, bound(if reverse { 3 } else { 1 }));
                    assert_eq!(range.end, bound(if reverse { 1 } else { 3 }));
                }
                let inverse = prepare_axis_inverse(&book, &target, DocumentVersion::new(3)?)?;
                book = book.candidate(&inverse, DocumentVersion::new(3)?)?;
                assert_eq!(
                    book.cell_content(origin)?,
                    Some(&CellContent::Formula(formula))
                );
            }
        }
    }
    Ok(())
}

#[test]
fn bounded_preimages_refuse_large_deletion_without_changing_legacy_replay() -> ResultTest {
    let base = fixture()?;
    let mut seed = base.snapshot_image()?.sheets.remove(0).sheet;
    seed.columns = (50..67)
        .map(|n| ColumnId::from_bytes([n; 16]))
        .collect::<Result<_, _>>()?;
    let cells = seed
        .columns
        .iter()
        .map(|column| {
            (
                CellAddress {
                    sheet: seed.id,
                    row: seed.rows[1],
                    column: *column,
                },
                CellValue::Text("a".repeat(32768)).into(),
            )
        })
        .collect::<Vec<_>>();
    let book = Workbook::hydrate(base.document_id(), base.version(), seed.clone(), cells)?;
    let hash = book.hash();
    let legacy = Operation::DeleteRows {
        sheet: seed.id,
        rows: vec![seed.rows[1]],
    };
    assert_eq!(
        prepare_axis_edit(&book, &legacy, DocumentVersion::new(2)?),
        Err(AxisEditError::Domain(DomainError::LimitExceeded))
    );
    assert_eq!(book.hash(), hash);
    assert!(book.candidate(&legacy, DocumentVersion::new(2)?).is_ok());
    Ok(())
}

#[test]
fn compensation_cannot_smuggle_unrelated_scalar_writes_or_invent_reactivation() -> ResultTest {
    let mut book = fixture()?;
    let removed = at(&book, 1, 0);
    let unrelated = at(&book, 0, 1);
    let operation = prepare_axis_edit(
        &book,
        &Operation::DeleteRows {
            sheet: removed.sheet,
            rows: vec![removed.row],
        },
        DocumentVersion::new(2)?,
    )?;
    let target = event(&book, operation)?;
    book.apply_event(&target)?;
    let inverse = prepare_axis_inverse(&book, &target, DocumentVersion::new(3)?)?;
    let mut bad = inverse.clone();
    if let Operation::AxisCompensation { change, .. } = &mut bad {
        change.writes.push(GuardedContentWrite {
            address: unrelated,
            previous: None,
            content: Some(CellValue::Number(7.).into()),
        });
    }
    assert!(book.candidate(&bad, DocumentVersion::new(3)?).is_err());
    let mut bad = inverse;
    if let Operation::AxisCompensation { change, .. } = &mut bad
        && let AxisDelta::Rows { inserted, .. } = &mut change.delta
    {
        inserted[0].row = RowId::from_bytes([90; 16])?;
    }
    assert!(book.candidate(&bad, DocumentVersion::new(3)?).is_err());
    Ok(())
}
