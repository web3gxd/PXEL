use spreadsheet_domain::workbook::*;
use spreadsheet_formula::{
    binder::bind,
    move_history::{MoveInverseError, plan_move_inverse, plan_move_inverse_cancellable},
    relocation::{MoveRequest, plan_move},
};

type ResultTest<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn fixture() -> ResultTest<Workbook> {
    Ok(Workbook::hydrate_sheets(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        [SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet".into(),
            rows: (10u128..16)
                .map(|n| RowId::from_bytes(n.to_be_bytes()))
                .collect::<Result<_, _>>()?,
            columns: (20u128..26)
                .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
                .collect::<Result<_, _>>()?,
        }],
        [],
    )?)
}
fn address(book: &Workbook, row: usize, column: usize) -> CellAddress {
    let sheet = &book.sheets()[0];
    CellAddress {
        sheet: sheet.id(),
        row: sheet.rows()[row],
        column: sheet.columns()[column],
    }
}
fn apply(book: &mut Workbook, operation: Operation, actor: u8) -> ResultTest<CommittedEvent> {
    let version = DocumentVersion::new(book.version().get() + 1)?;
    let candidate = book.candidate(&operation, version)?;
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes(u128::from(version.get()).to_be_bytes())?,
        principal: EventPrincipal::User(ActorId::from_bytes([actor; 16])?),
        version,
        operation,
        state_hash: candidate.hash(),
    };
    *book = candidate;
    Ok(event)
}
fn formula(
    book: &mut Workbook,
    origin: CellAddress,
    text: &str,
    actor: u8,
) -> ResultTest<CommittedEvent> {
    let formula = bind(text, book, origin)?;
    let previous = book.cell_content(origin)?.cloned();
    apply(
        book,
        Operation::CellWrite {
            address: origin,
            content: Some(CellContent::Formula(formula)),
            previous,
            kind: Some(CellWriteKind::SetFormula),
            undoes: None,
            undoes_version: None,
        },
        actor,
    )
}
fn moved() -> ResultTest<(Workbook, Workbook, CommittedEvent)> {
    let mut book = fixture()?;
    let from = address(&book, 1, 1);
    let to = address(&book, 1, 3);
    apply(
        &mut book,
        Operation::SetCellValue {
            address: from,
            value: CellValue::Number(7.0),
        },
        3,
    )?;
    let external = address(&book, 5, 5);
    formula(&mut book, external, "=$B$2*2", 3)?;
    let destination_observer = address(&book, 4, 4);
    formula(&mut book, destination_observer, "=$D$2+1", 3)?;
    let before = book.clone();
    let plan = plan_move(
        &book,
        MoveRequest {
            source: from,
            destination: to,
            rows: 1,
            columns: 1,
        },
    )?;
    let target = apply(
        &mut book,
        Operation::MoveRange {
            source: from,
            destination: to,
            rows: 1,
            columns: 1,
            writes: plan.writes,
        },
        3,
    )?;
    Ok((before, book, target))
}

#[test]
fn inverse_restores_frozen_preimages_and_inbound_ast_without_source_reread() -> ResultTest {
    let (before, book, target) = moved()?;
    let hash = book.hash();
    let plan = plan_move_inverse(&book, &target, &[])?;
    assert_eq!(plan.input_hash, hash);
    assert_eq!(plan.input_version, book.version());
    assert_eq!(plan.target_operation_id, target.operation_id);
    assert_eq!(plan.target_version, target.version);
    assert_eq!(plan.writes.len(), 3);
    for write in plan.writes {
        assert_eq!(write.content.as_ref(), before.cell_content(write.address)?);
        assert_eq!(write.previous.as_ref(), book.cell_content(write.address)?);
    }
    assert_eq!(book.hash(), hash);
    Ok(())
}

#[test]
fn existing_destination_reference_and_unrelated_new_formula_do_not_block_inverse() -> ResultTest {
    let (_, mut book, target) = moved()?;
    let origin = address(&book, 4, 5);
    let event = formula(&mut book, origin, "=A1+1", 4)?;
    assert!(plan_move_inverse(&book, &target, &[event]).is_ok());
    Ok(())
}

#[test]
fn new_scalar_and_interior_reversed_range_references_block_inverse() -> ResultTest {
    for text in ["=$D$2", "=SUM(A1:E3)", "=SUM(E3:A1)"] {
        let (_, mut book, target) = moved()?;
        let origin = address(&book, 4, 5);
        let event = formula(&mut book, origin, text, 4)?;
        assert!(
            matches!(
                plan_move_inverse(&book, &target, &[event]),
                Err(MoveInverseError::Conflict)
            ),
            "{text}"
        );
    }
    Ok(())
}

#[test]
fn foreign_same_value_write_is_a_barrier_even_if_projection_matches() -> ResultTest {
    let (_, mut book, target) = moved()?;
    let destination = address(&book, 1, 3);
    let event = apply(
        &mut book,
        Operation::SetCellValue {
            address: destination,
            value: CellValue::Number(7.0),
        },
        4,
    )?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[event]),
        Err(MoveInverseError::Conflict)
    ));
    Ok(())
}

#[test]
fn missing_or_wrong_document_history_is_not_accepted() -> ResultTest {
    let (_, mut book, target) = moved()?;
    let origin = address(&book, 4, 5);
    let mut event = formula(&mut book, origin, "=A1", 4)?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[]),
        Err(MoveInverseError::InvalidHistory)
    ));
    event.document_id = DocumentId::from_bytes([9; 16])?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[event]),
        Err(MoveInverseError::InvalidHistory)
    ));
    Ok(())
}

#[test]
fn removed_new_reference_and_legacy_copy_formula_remain_barriers() -> ResultTest {
    let (_, mut book, target) = moved()?;
    let origin = address(&book, 4, 5);
    let referenced = formula(&mut book, origin, "=D2", 4)?;
    let cleared = apply(&mut book, Operation::ClearCell { address: origin }, 4)?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[referenced, cleared]),
        Err(MoveInverseError::Conflict)
    ));

    let (_, mut book, target) = moved()?;
    let copied = bind("=SUM(A1:E3)", &book, origin)?;
    let copy = apply(
        &mut book,
        Operation::CopyRange {
            writes: vec![ContentWrite {
                address: origin,
                content: Some(CellContent::Formula(copied)),
            }],
        },
        4,
    )?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[copy]),
        Err(MoveInverseError::Conflict)
    ));
    Ok(())
}

#[test]
fn range_authored_and_compensation_formula_contents_are_inspected() -> ResultTest {
    for authored in [true, false] {
        let (_, mut book, target) = moved()?;
        let origin = address(&book, 4, 5);
        let referencing = bind("=SUM(A1:E3)", &book, origin)?;
        // Domain routing fixture: compensation provenance is the journal's
        // responsibility and is deliberately not an admission claim here.
        let event = apply(
            &mut book,
            Operation::RangeWrite {
                writes: vec![GuardedContentWrite {
                    address: origin,
                    content: Some(CellContent::Formula(referencing)),
                    previous: None,
                }],
                kind: authored.then_some(RangeWriteKind::Copy),
                undoes: (!authored).then_some(target.operation_id),
                undoes_version: (!authored).then_some(target.version),
            },
            4,
        )?;
        assert!(matches!(
            plan_move_inverse(&book, &target, &[event]),
            Err(MoveInverseError::Conflict)
        ));
    }
    Ok(())
}

#[test]
fn terminal_hash_reordered_tail_and_axes_changes_are_refused() -> ResultTest {
    let (_, mut book, target) = moved()?;
    let origin = address(&book, 4, 5);
    let first = formula(&mut book, origin, "=A1", 4)?;
    let second = formula(&mut book, origin, "=A1+2", 4)?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[second.clone(), first.clone()]),
        Err(MoveInverseError::InvalidHistory)
    ));
    let mut corrupt = second;
    corrupt.state_hash = [0; 32];
    assert!(matches!(
        plan_move_inverse(&book, &target, &[first, corrupt]),
        Err(MoveInverseError::InvalidHistory)
    ));

    let (_, mut book, target) = moved()?;
    let event = apply(
        &mut book,
        Operation::ExtendRows {
            sheet: origin.sheet,
            rows: vec![RowId::from_bytes([99; 16])?],
        },
        3,
    )?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[event]),
        Err(MoveInverseError::Conflict)
    ));
    Ok(())
}

#[test]
fn overlap_inverse_restores_overwritten_formula_exactly() -> ResultTest {
    let mut book = fixture()?;
    let source = address(&book, 0, 0);
    let destination = address(&book, 0, 1);
    formula(&mut book, source, "=E5+1", 3)?;
    formula(&mut book, destination, "=E5+2", 3)?;
    let overwritten = address(&book, 0, 2);
    formula(&mut book, overwritten, "=E5+3", 3)?;
    let before = book.clone();
    let planned = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 2,
        },
    )?;
    let target = apply(
        &mut book,
        Operation::MoveRange {
            source,
            destination,
            rows: 1,
            columns: 2,
            writes: planned.writes,
        },
        3,
    )?;
    let inverse = plan_move_inverse(&book, &target, &[])?;
    let restored = book.candidate(
        &Operation::RangeWrite {
            writes: inverse.writes,
            kind: None,
            undoes: Some(target.operation_id),
            undoes_version: Some(target.version),
        },
        DocumentVersion::new(book.version().get() + 1)?,
    )?;
    for address in [source, destination, overwritten] {
        assert_eq!(
            restored.cell_content(address)?,
            before.cell_content(address)?
        );
    }
    Ok(())
}

#[test]
fn cancellation_and_tail_budget_leave_input_unchanged() -> ResultTest {
    let (_, book, target) = moved()?;
    let hash = book.hash();
    assert!(matches!(
        plan_move_inverse_cancellable(&book, &target, &[], || true),
        Err(MoveInverseError::Cancelled)
    ));
    let mut callbacks = 0;
    assert!(matches!(
        plan_move_inverse_cancellable(&book, &target, &[], || {
            callbacks += 1;
            callbacks == 3
        }),
        Err(MoveInverseError::Cancelled)
    ));
    assert!(matches!(
        plan_move_inverse(&book, &target, &vec![target.clone(); 10_001]),
        Err(MoveInverseError::LimitExceeded)
    ));
    assert_eq!(book.hash(), hash);
    Ok(())
}

#[test]
fn cross_sheet_range_covering_a_rewritten_external_formula_is_a_conflict() -> ResultTest {
    let mut book = fixture()?;
    let seed = SheetSeed {
        id: SheetId::from_bytes([8; 16])?,
        name: "Remote".into(),
        rows: (100u128..104)
            .map(|n| RowId::from_bytes(n.to_be_bytes()))
            .collect::<Result<_, _>>()?,
        columns: (200u128..204)
            .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let external = CellAddress {
        sheet: seed.id,
        row: seed.rows[1],
        column: seed.columns[1],
    };
    apply(&mut book, Operation::AddSheet { sheet: seed }, 3)?;
    formula(&mut book, external, "=Sheet!B2", 3)?;
    let source = address(&book, 1, 1);
    let destination = address(&book, 1, 3);
    let planned = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    )?;
    let target = apply(
        &mut book,
        Operation::MoveRange {
            source,
            destination,
            rows: 1,
            columns: 1,
            writes: planned.writes,
        },
        3,
    )?;
    let origin = address(&book, 5, 5);
    let event = formula(&mut book, origin, "=SUM(Remote!C3:A1)", 4)?;
    assert!(matches!(
        plan_move_inverse(&book, &target, &[event]),
        Err(MoveInverseError::Conflict)
    ));
    Ok(())
}

#[test]
fn disjoint_ranges_are_allowed_and_total_inspection_has_a_real_budget() -> ResultTest {
    let (_, mut book, target) = moved()?;
    let origin = address(&book, 4, 5);
    let event = formula(&mut book, origin, "=SUM(A3:E4)", 4)?;
    assert!(plan_move_inverse(&book, &target, &[event]).is_ok());

    let (_, mut book, target) = moved()?;
    let previous = book.cell_content(origin)?.cloned();
    let mut text = String::from("=A1");
    for _ in 0..55 {
        text.push_str("+1");
    }
    let bound = bind(&text, &book, origin)?;
    let operation = Operation::CellWrite {
        address: origin,
        content: Some(CellContent::Formula(bound)),
        previous,
        kind: Some(CellWriteKind::SetFormula),
        undoes: None,
        undoes_version: None,
    };
    let first = apply(&mut book, operation, 4)?;
    let mut tail = Vec::with_capacity(10_000);
    tail.push(first.clone());
    for _ in 1..10_000 {
        let mut operation = first.operation.clone();
        if let Operation::CellWrite { previous, .. } = &mut operation {
            *previous = book.cell_content(origin)?.cloned();
        }
        tail.push(apply(&mut book, operation, 4)?);
    }
    assert!(matches!(
        plan_move_inverse(&book, &target, &tail),
        Err(MoveInverseError::LimitExceeded)
    ));
    Ok(())
}
