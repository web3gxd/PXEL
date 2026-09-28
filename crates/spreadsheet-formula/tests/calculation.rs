use spreadsheet_domain::{formula as f, workbook::*};
use spreadsheet_formula::{
    EvaluationError, FormulaError, Value,
    binder::bind,
    calculation::{CalculationError, CalculationLimits, CalculationProjection},
};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn address(row: u32, column: u8) -> Result<CellAddress, DomainError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: RowId::from_bytes((u128::from(row) + 10).to_be_bytes())?,
        column: ColumnId::from_bytes([column + 1; 16])?,
    })
}
fn seed(rows: u32, columns: u8) -> Result<SheetSeed, DomainError> {
    Ok(SheetSeed {
        id: SheetId::from_bytes([1; 16])?,
        name: "Sheet1".into(),
        rows: (0..rows)
            .map(|r| address(r, 0).map(|a| a.row))
            .collect::<Result<_, _>>()?,
        columns: (0..columns)
            .map(|c| address(0, c).map(|a| a.column))
            .collect::<Result<_, _>>()?,
    })
}
fn fixture(
    cells: Vec<(CellAddress, CellContent)>,
    rows: u32,
    columns: u8,
) -> Result<Workbook, DomainError> {
    Workbook::from_content_fixture(
        DocumentId::from_bytes([2; 16])?,
        DocumentVersion::new(1)?,
        seed(rows, columns)?,
        cells,
    )
}
fn dependent(
    origin: CellAddress,
    target: CellAddress,
) -> Result<f::Formula, Box<dyn std::error::Error>> {
    Ok(f::Formula::new(
        "=reference+1".into(),
        "=reference+1".into(),
        f::BoundAst::new(
            1,
            f::NodeId(2),
            vec![
                f::Node::Cell(f::BoundCellReference {
                    target_sheet: target.sheet,
                    target_row: target.row,
                    target_column: target.column,
                    row_mode: f::AxisReferenceMode::Absolute,
                    column_mode: f::AxisReferenceMode::Absolute,
                }),
                f::Node::Number(1.0),
                f::Node::Binary {
                    op: f::BinaryOp::Add,
                    left: f::NodeId(0),
                    right: f::NodeId(1),
                },
            ],
        )?,
        origin,
        DocumentVersion::new(1)?,
    )?)
}
fn edit(book: &mut Workbook, projection: &mut CalculationProjection, op: Operation) -> TestResult {
    let candidate = book.candidate(&op, DocumentVersion::new(book.version().get() + 1)?)?;
    let delta = projection.prepare(&candidate, &op, CalculationLimits::default())?;
    projection.apply(delta);
    *book = candidate;
    Ok(())
}
fn formula(
    book: &Workbook,
    at: CellAddress,
    source: &str,
) -> Result<Operation, Box<dyn std::error::Error>> {
    Ok(Operation::SetCellFormula {
        address: at,
        formula: bind(source, book, at)?,
    })
}
#[test]
fn ten_thousand_chain_evaluates_real_values_without_recursing_the_graph() -> TestResult {
    let mut cells = vec![(address(0, 0)?, CellContent::Value(CellValue::Number(1.0)))];
    for row in 1..=10000 {
        cells.push((
            address(row, 0)?,
            CellContent::Formula(dependent(address(row, 0)?, address(row - 1, 0)?)?),
        ));
    }
    let mut book = fixture(cells, 10001, 1)?;
    let mut p = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    let first = p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(first.evaluated_cells, 10000);
    assert_eq!(
        p.results().get(&address(10000, 0)?),
        Some(&Value::Number(10001.0))
    );
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(0, 0)?,
            value: CellValue::Number(2.0),
        },
    )?;
    let next = p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(next.evaluated_cells, 10000);
    assert_eq!(next.input_version, book.version());
    assert_eq!(
        p.results().get(&address(10000, 0)?),
        Some(&Value::Number(10002.0))
    );
    Ok(())
}
#[test]
fn changing_one_of_ten_thousand_independent_inputs_evaluates_one_formula() -> TestResult {
    let mut cells = Vec::new();
    for row in 0..10000 {
        cells.push((address(row, 0)?, CellContent::Value(CellValue::Number(1.0))));
        cells.push((
            address(row, 1)?,
            CellContent::Formula(dependent(address(row, 1)?, address(row, 0)?)?),
        ));
    }
    let mut book = fixture(cells, 10000, 2)?;
    let mut p = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(5000, 0)?,
            value: CellValue::Number(7.0),
        },
    )?;
    let patch = p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(patch.evaluated_cells, 1);
    assert_eq!(patch.changed_results.len(), 1);
    assert_eq!(patch.dirty_cells, 2);
    assert!(patch.graph_work_used < 20);
    assert_eq!(
        p.results().get(&address(5000, 1)?),
        Some(&Value::Number(8.0))
    );
    Ok(())
}
#[test]
fn replacement_removes_edges_and_formula_to_value_removes_stale_cache() -> TestResult {
    let mut book = fixture(vec![], 1, 4)?;
    let op = formula(&book, address(0, 1)?, "=A1+1")?;
    book = book.candidate(&op, DocumentVersion::new(2)?)?;
    let op = formula(&book, address(0, 3)?, "=B1+1")?;
    book = book.candidate(&op, DocumentVersion::new(3)?)?;
    let mut p = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    let op = formula(&book, address(0, 1)?, "=C1+1")?;
    edit(&mut book, &mut p, op)?;
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(0, 0)?,
            value: CellValue::Number(9.0),
        },
    )?;
    assert_eq!(
        p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?
            .evaluated_cells,
        0
    );
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(0, 1)?,
            value: CellValue::Number(20.0),
        },
    )?;
    let patch = p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(patch.evaluated_cells, 1);
    assert!(patch.cleared_results.contains(&address(0, 1)?));
    assert!(!p.results().contains_key(&address(0, 1)?));
    assert_eq!(p.results().get(&address(0, 3)?), Some(&Value::Number(21.0)));
    Ok(())
}
#[test]
fn newly_inserted_formula_in_range_gets_topological_edges_and_cycles() -> TestResult {
    let mut book = fixture(
        vec![
            (address(0, 1)?, CellContent::Value(CellValue::Number(1.0))),
            (address(2, 1)?, CellContent::Value(CellValue::Number(3.0))),
        ],
        3,
        3,
    )?;
    let op = formula(&book, address(0, 0)?, "=SUM(B1:B3)")?;
    book = book.candidate(&op, DocumentVersion::new(2)?)?;
    let mut p = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    let op = formula(&book, address(1, 1)?, "=C1+1")?;
    edit(&mut book, &mut p, op)?;
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(0, 2)?,
            value: CellValue::Number(5.0),
        },
    )?;
    let patch = p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(patch.evaluated_cells, 2);
    assert_eq!(p.results().get(&address(0, 0)?), Some(&Value::Number(10.0)));
    let op = formula(&book, address(1, 1)?, "=A1")?;
    edit(&mut book, &mut p, op)?;
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(
        p.results().get(&address(0, 0)?),
        Some(&Value::Error(FormulaError::Cycle))
    );
    assert_eq!(
        p.results().get(&address(1, 1)?),
        Some(&Value::Error(FormulaError::Cycle))
    );
    Ok(())
}
#[test]
fn failed_or_cancelled_calculation_does_not_publish_partial_cache() -> TestResult {
    let mut book = fixture(
        vec![
            (
                address(0, 1)?,
                CellContent::Formula(dependent(address(0, 1)?, address(0, 0)?)?),
            ),
            (
                address(0, 2)?,
                CellContent::Formula(dependent(address(0, 2)?, address(0, 1)?)?),
            ),
        ],
        1,
        3,
    )?;
    let mut p = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    let old = p.results().clone();
    let old_version = p.calculated_version();
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(0, 0)?,
            value: CellValue::Number(5.0),
        },
    )?;
    let failed = p.calculate(
        &book,
        CalculationLimits {
            evaluation_work: 4,
            ..CalculationLimits::default()
        },
        &AtomicBool::new(false),
    );
    assert_eq!(
        failed,
        Err(CalculationError::Evaluation(
            EvaluationError::CalculationLimitExceeded
        ))
    );
    assert_eq!(p.results(), &old);
    assert_eq!(p.calculated_version(), old_version);
    assert!(p.dirty_count() > 0);
    assert_eq!(
        p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(true)),
        Err(CalculationError::Evaluation(EvaluationError::Cancelled))
    );
    assert_eq!(p.results(), &old);
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(p.results().get(&address(0, 2)?), Some(&Value::Number(7.0)));
    let old = p.results().clone();
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(0, 1)?,
            value: CellValue::Number(20.0),
        },
    )?;
    assert_eq!(p.results(), &old);
    assert!(
        p.calculate(
            &book,
            CalculationLimits {
                evaluation_work: 0,
                ..CalculationLimits::default()
            },
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(p.results(), &old);
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert!(!p.results().contains_key(&address(0, 1)?));
    assert_eq!(p.results().get(&address(0, 2)?), Some(&Value::Number(21.0)));
    Ok(())
}

#[test]
fn coalesced_formula_and_value_changes_equal_sequential_calculation() -> TestResult {
    let mut book = fixture(vec![], 2, 3)?;
    let initial = formula(&book, address(0, 1)?, "=A1+1")?;
    book = book.candidate(&initial, DocumentVersion::new(2)?)?;
    let mut sequential = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    let mut coalesced = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    sequential.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    coalesced.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    let mut changed = BTreeSet::new();
    let op = Operation::SetCellValue {
        address: address(0, 0)?,
        value: CellValue::Number(9.0),
    };
    changed.insert(address(0, 0)?);
    edit(&mut book, &mut sequential, op)?;
    sequential.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    let op = formula(&book, address(0, 1)?, "=SUM(A1:A2)")?;
    changed.insert(address(0, 1)?);
    edit(&mut book, &mut sequential, op)?;
    sequential.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    let op = Operation::SetCellValue {
        address: address(1, 0)?,
        value: CellValue::Number(3.0),
    };
    changed.insert(address(1, 0)?);
    edit(&mut book, &mut sequential, op)?;
    sequential.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(book.version().get(), 5);
    coalesced.reconcile(&book, &changed, CalculationLimits::default())?;
    let patch =
        coalesced.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(patch.input_version.get(), 5);
    assert_eq!(patch.evaluated_cells, 1);
    assert_eq!(coalesced.results(), sequential.results());
    assert_eq!(coalesced.result_hash()?, sequential.result_hash()?);
    assert_eq!(coalesced.result_bytes(), sequential.result_bytes());
    Ok(())
}

#[test]
fn expanded_result_memory_cap_is_atomic_and_replacement_subtracts_previous_size() -> TestResult {
    let mut book = fixture(
        vec![(
            address(0, 0)?,
            CellContent::Value(CellValue::Text("x".repeat(80))),
        )],
        1,
        3,
    )?;
    for column in [1, 2] {
        let op = formula(&book, address(0, column)?, "=A1&A1")?;
        book = book.candidate(&op, DocumentVersion::new(book.version().get() + 1)?)?;
    }
    let mut p = CalculationProjection::from_workbook(&book, CalculationLimits::default())?;
    let result = p.calculate(
        &book,
        CalculationLimits {
            max_result_bytes: 500,
            ..CalculationLimits::default()
        },
        &AtomicBool::new(false),
    );
    assert_eq!(
        result,
        Err(CalculationError::Evaluation(
            EvaluationError::CalculationLimitExceeded
        ))
    );
    assert!(p.results().is_empty());
    assert_eq!(p.result_bytes(), 0);
    assert!(p.dirty_count() > 0);
    assert!(p.result_hash().is_err());
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(p.result_bytes(), 704);
    edit(
        &mut book,
        &mut p,
        Operation::SetCellValue {
            address: address(0, 0)?,
            value: CellValue::Text("y".repeat(80)),
        },
    )?;
    p.calculate(
        &book,
        CalculationLimits {
            max_result_bytes: 704,
            ..CalculationLimits::default()
        },
        &AtomicBool::new(false),
    )?;
    assert_eq!(p.result_bytes(), 704);
    edit(
        &mut book,
        &mut p,
        Operation::ClearCell {
            address: address(0, 1)?,
        },
    )?;
    assert_eq!(p.result_bytes(), 704);
    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))?;
    assert_eq!(p.result_bytes(), 352);
    Ok(())
}
