use spreadsheet_domain::{formula as d, workbook::*};
use spreadsheet_formula::{
    EvaluationBudget, EvaluationError, FormulaError, Value,
    bound::{bound_dependencies, evaluate_bound},
};
use std::collections::BTreeMap;
type ResultTest = Result<(), Box<dyn std::error::Error>>;
fn row(n: u8) -> Result<RowId, DomainError> {
    RowId::from_bytes([n; 16])
}
fn address(n: u8, column: u8) -> Result<CellAddress, DomainError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: row(n)?,
        column: ColumnId::from_bytes([column; 16])?,
    })
}
fn target(n: u8) -> Result<d::BoundCellReference, DomainError> {
    let a = address(n, 20)?;
    Ok(d::BoundCellReference {
        target_sheet: a.sheet,
        target_row: a.row,
        target_column: a.column,
        row_mode: d::AxisReferenceMode::Relative { original_delta: 0 },
        column_mode: d::AxisReferenceMode::Absolute,
    })
}
fn book(reverse: bool) -> Result<Workbook, Box<dyn std::error::Error>> {
    let mut rows = (2..8).map(row).collect::<Result<Vec<_>, _>>()?;
    if reverse {
        rows.reverse()
    }
    let operation = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: SheetId::from_bytes([1; 16])?,
            name: "Renamable sheet".into(),
            rows,
            columns: vec![
                ColumnId::from_bytes([20; 16])?,
                ColumnId::from_bytes([21; 16])?,
            ],
        },
    };
    let mut w = Workbook::empty(DocumentId::from_bytes([30; 16])?)
        .candidate(&operation, DocumentVersion::new(1)?)?;
    for (n, value) in [
        (2, CellValue::Number(1.0)),
        (3, CellValue::Text("99".into())),
        (4, CellValue::Boolean(true)),
        (6, CellValue::Number(3.0)),
        (7, CellValue::Number(0.0)),
    ] {
        w = w.candidate(
            &Operation::SetCellValue {
                address: address(n, 20)?,
                value,
            },
            DocumentVersion::new(w.version().get() + 1)?,
        )?;
    }
    Ok(w)
}
fn formula(nodes: Vec<d::Node>) -> Result<d::Formula, Box<dyn std::error::Error>> {
    let root = d::NodeId(u32::try_from(
        nodes.len().checked_sub(1).ok_or("empty nodes")?,
    )?);
    Ok(d::Formula::new(
        "=deliberately_not_reparsed".into(),
        "=also_not_reparsed".into(),
        d::BoundAst::new(1, root, nodes)?,
        address(2, 21)?,
        DocumentVersion::new(1)?,
    )?)
}
fn aggregate(name: &str) -> Result<d::Formula, Box<dyn std::error::Error>> {
    formula(vec![
        d::Node::Range(d::BoundRangeReference {
            start: target(2)?,
            end: target(7)?,
        }),
        d::Node::Call {
            name: d::FunctionName::new(name.into())?,
            arguments: vec![d::NodeId(0)],
        },
    ])
}
#[test]
fn stable_targets_survive_changed_axis_positions_and_copy_flags_do_not_retarget() -> ResultTest {
    for reverse in [false, true] {
        let w = book(reverse)?;
        let r = target(6)?;
        for mode in [
            d::AxisReferenceMode::Absolute,
            d::AxisReferenceMode::Relative {
                original_delta: -100,
            },
        ] {
            let f = formula(vec![d::Node::Cell(d::BoundCellReference {
                row_mode: mode,
                ..r
            })])?;
            assert_eq!(
                evaluate_bound(&f, &w, &BTreeMap::new(), &mut EvaluationBudget::default())?,
                Value::Number(3.0)
            );
        }
    }
    Ok(())
}
#[test]
fn ranges_keep_reference_coercion_and_blank_cells_and_calculated_errors() -> ResultTest {
    let w = book(false)?;
    for (name, expected) in [
        ("SUM", 4.0),
        ("AVERAGE", 4.0 / 3.0),
        ("MIN", 0.0),
        ("MAX", 3.0),
    ] {
        assert_eq!(
            evaluate_bound(
                &aggregate(name)?,
                &w,
                &BTreeMap::new(),
                &mut EvaluationBudget::default()
            )?,
            Value::Number(expected)
        );
    }
    let text = formula(vec![
        d::Node::Cell(target(3)?),
        d::Node::Call {
            name: d::FunctionName::new("SUM".into())?,
            arguments: vec![d::NodeId(0)],
        },
    ])?;
    assert_eq!(
        evaluate_bound(
            &text,
            &w,
            &BTreeMap::new(),
            &mut EvaluationBudget::default()
        )?,
        Value::Number(0.0)
    );
    let cached = BTreeMap::from([(address(5, 20)?, Value::Number(8.0))]);
    assert_eq!(
        evaluate_bound(
            &aggregate("SUM")?,
            &w,
            &cached,
            &mut EvaluationBudget::default()
        )?,
        Value::Number(12.0)
    );
    let cached = BTreeMap::from([(address(5, 20)?, Value::Error(FormulaError::Na))]);
    assert_eq!(
        evaluate_bound(
            &aggregate("SUM")?,
            &w,
            &cached,
            &mut EvaluationBudget::default()
        )?,
        Value::Error(FormulaError::Na)
    );
    Ok(())
}
#[test]
fn missing_stable_targets_produce_ref_and_missing_origin_is_invalid() -> ResultTest {
    let w = book(false)?;
    let f = formula(vec![d::Node::Cell(target(99)?)])?;
    assert_eq!(
        evaluate_bound(&f, &w, &BTreeMap::new(), &mut EvaluationBudget::default())?,
        Value::Error(FormulaError::Ref)
    );
    assert!(bound_dependencies(&f, &w)?.direct.is_empty());
    let f = formula(vec![d::Node::Range(d::BoundRangeReference {
        start: target(2)?,
        end: target(99)?,
    })])?;
    assert_eq!(
        evaluate_bound(&f, &w, &BTreeMap::new(), &mut EvaluationBudget::default())?,
        Value::Error(FormulaError::Ref)
    );
    let f = d::Formula::new(
        "=1".into(),
        "=1".into(),
        d::BoundAst::new(1, d::NodeId(0), vec![d::Node::Number(1.0)])?,
        address(99, 21)?,
        DocumentVersion::new(1)?,
    )?;
    assert!(matches!(
        evaluate_bound(&f, &w, &BTreeMap::new(), &mut EvaluationBudget::default()),
        Err(EvaluationError::InvalidAst)
    ));
    Ok(())
}
#[test]
fn dependencies_are_stable_direct_ids_and_compact_current_rectangles() -> ResultTest {
    let w = book(false)?;
    let f = formula(vec![
        d::Node::Cell(target(2)?),
        d::Node::Cell(target(2)?),
        d::Node::Range(d::BoundRangeReference {
            start: target(7)?,
            end: target(2)?,
        }),
        d::Node::Call {
            name: d::FunctionName::new("SUM".into())?,
            arguments: vec![d::NodeId(0), d::NodeId(1), d::NodeId(2)],
        },
    ])?;
    let deps = bound_dependencies(&f, &w)?;
    assert_eq!(deps.direct.len(), 1);
    assert!(deps.direct.contains(&address(2, 20)?));
    assert_eq!(deps.ranges.len(), 1);
    let range = deps.ranges.first().ok_or("missing range")?;
    assert_eq!(
        (
            range.first_row,
            range.last_row,
            range.first_column,
            range.last_column
        ),
        (0, 5, 0, 0)
    );
    assert_eq!(range.formula, f.origin());
    Ok(())
}
#[test]
fn invalid_calculated_values_are_controlled_errors_and_budget_is_preserved() -> ResultTest {
    let w = book(false)?;
    let f = formula(vec![d::Node::Cell(target(5)?)])?;
    for invalid in [
        Value::Number(f64::NAN),
        Value::Text("x".repeat(32769)),
        Value::Array {
            rows: 2,
            columns: 2,
            values: vec![Value::Blank],
        },
    ] {
        let results = BTreeMap::from([(address(5, 20)?, invalid)]);
        assert!(matches!(
            evaluate_bound(&f, &w, &results, &mut EvaluationBudget::default()),
            Err(EvaluationError::InvalidResolverResult)
        ));
    }
    assert!(matches!(
        evaluate_bound(
            &aggregate("SUM")?,
            &w,
            &BTreeMap::new(),
            &mut EvaluationBudget::new(0)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    ));
    let mut budget = EvaluationBudget::default();
    budget.cancel();
    assert!(matches!(
        evaluate_bound(&f, &w, &BTreeMap::new(), &mut budget),
        Err(EvaluationError::Cancelled)
    ));
    Ok(())
}
