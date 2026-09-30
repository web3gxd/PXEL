use proptest::prelude::*;
use spreadsheet_formula::*;
use std::collections::BTreeMap;

struct Table(BTreeMap<(u32, u32), Value>);
fn value(source: &str, table: &Table) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(evaluate(
        &parse(source)?.ast,
        table,
        &mut EvaluationBudget::default(),
    )?)
}
impl ReferenceResolver for Table {
    fn resolve_cell(&self, cell: &CellReference) -> Value {
        self.0
            .get(&(cell.row, cell.column))
            .cloned()
            .unwrap_or(Value::Blank)
    }
    fn visit_range(
        &self,
        range: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        for row in range.start.row.min(range.end.row)..=range.start.row.max(range.end.row) {
            for column in
                range.start.column.min(range.end.column)..=range.start.column.max(range.end.column)
            {
                visitor(self.resolve_cell(&CellReference {
                    row,
                    column,
                    ..range.start.clone()
                }))?;
            }
        }
        Ok(())
    }
}

#[test]
fn duplicate_keys_scalar_results_and_index_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let table = Table(BTreeMap::from([
        ((0, 0), Value::Number(7.)),
        ((1, 0), Value::Number(7.)),
        ((2, 0), Value::Number(13.)),
        ((0, 1), Value::Text("12".into())),
        ((1, 1), Value::Boolean(true)),
        ((2, 1), Value::Blank),
    ]));
    for (source, expected) in [
        ("=VLOOKUP(7,A1:B3,2,FALSE)", Value::Text("12".into())),
        ("=VLOOKUP(7,A1:B3,2,TRUE)", Value::Boolean(true)),
        ("=SUM(VLOOKUP(7,A1:B3,2,FALSE))", Value::Number(12.)),
        ("=COUNT(VLOOKUP(7,A1:B3,2,FALSE))", Value::Number(1.)),
        ("=VLOOKUP(13,A1:B3,2,FALSE)", Value::Number(0.)),
        ("=COUNTA(VLOOKUP(13,A1:B3,2,FALSE))", Value::Number(1.)),
        ("=VLOOKUP(7,B3:A1,2,FALSE)", Value::Text("12".into())),
        ("=VLOOKUP(7,A1:B3,2.9,FALSE)", Value::Text("12".into())),
        (
            "=VLOOKUP(7,A1:B3,0.9,FALSE)",
            Value::Error(FormulaError::Value),
        ),
        (
            "=VLOOKUP(7,A1:B3,1E100,FALSE)",
            Value::Error(FormulaError::Ref),
        ),
        (
            "=VLOOKUP(7,A1:B3,\"2\",\"FALSE\")",
            Value::Text("12".into()),
        ),
        ("=VLOOKUP(8,A1:B3,2,)", Value::Error(FormulaError::Na)),
        ("=VLOOKUP(8,A1:B3,2)", Value::Boolean(true)),
    ] {
        assert_eq!(value(source, &table)?, expected, "{source}");
    }
    Ok(())
}

#[test]
fn selected_values_and_errors_are_not_reference_coercions() -> Result<(), Box<dyn std::error::Error>>
{
    for selected in [
        Value::Blank,
        Value::Number(-0.),
        Value::Number(f64::INFINITY),
        Value::Boolean(true),
        Value::Text("text".into()),
        Value::Error(FormulaError::Div0),
    ] {
        let table = Table(BTreeMap::from([
            ((0, 0), Value::Number(7.)),
            ((0, 1), selected.clone()),
            ((1, 0), Value::Error(FormulaError::Na)),
            ((1, 1), Value::Error(FormulaError::Ref)),
        ]));
        let expected = match selected {
            Value::Blank => Value::Number(0.),
            Value::Number(n) if !n.is_finite() => Value::Error(FormulaError::Num),
            Value::Number(0.) => Value::Number(0.),
            other => other,
        };
        assert_eq!(value("=VLOOKUP(7,A1:B2,2,FALSE)", &table)?, expected);
        assert_eq!(
            value("=VLOOKUP(7,A1:B2,2,TRUE)", &table)?,
            Value::Error(FormulaError::Na)
        );
    }
    let table = Table(BTreeMap::from([
        ((0, 0), Value::Text("Alpha".into())),
        ((0, 1), Value::Number(21.)),
    ]));
    assert_eq!(
        value("=VLOOKUP(\"a*\",A1:B1,2,FALSE)", &table)?,
        Value::Number(21.)
    );
    assert_eq!(
        value("=VLOOKUP(\"a*\",A1:B1,2,TRUE)", &table)?,
        Value::Error(FormulaError::Na)
    );
    Ok(())
}

#[test]
fn table_limits_remain_typed_through_iferror() -> Result<(), Box<dyn std::error::Error>> {
    for (selected, expected) in [
        (
            Value::Text("x".repeat(40000)),
            EvaluationError::CalculationLimitExceeded,
        ),
        (
            Value::Array {
                rows: 1,
                columns: 1,
                values: vec![Value::Number(1.)],
            },
            EvaluationError::UnsupportedCompatibilityFeature,
        ),
    ] {
        let table = Table(BTreeMap::from([
            ((0, 0), Value::Number(7.)),
            ((0, 1), selected),
        ]));
        assert_eq!(
            evaluate(
                &parse("=IFERROR(VLOOKUP(7,A1:B1,2,FALSE),99)")?.ast,
                &table,
                &mut EvaluationBudget::default()
            ),
            Err(expected)
        );
    }
    let table = Table(BTreeMap::from([((0, 0), Value::Number(7.))]));
    for source in [
        "=IFERROR(VLOOKUP(7,A1:B50001,1,FALSE),99)",
        "=IFERROR(HLOOKUP(7,A1:XFD7,1,FALSE),99)",
    ] {
        assert!(matches!(
            parse(source),
            Err(ParseError {
                code: ParseErrorCode::LimitExceeded,
                ..
            })
        ));
    }
    let ast = parse("=IFERROR(VLOOKUP(7,A1:B100,1,FALSE),99)")?.ast;
    assert_eq!(
        evaluate(&ast, &table, &mut EvaluationBudget::new(30)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let mut cancelled = EvaluationBudget::default();
    cancelled.cancel();
    assert_eq!(
        evaluate(&ast, &table, &mut cancelled),
        Err(EvaluationError::Cancelled)
    );
    Ok(())
}

proptest! {
    #[test]
    fn transposition_preserves_lookup_and_matches_independent_oracle(mut keys in prop::collection::vec(-20i16..20,1..25), lookup in -25i16..25, approximate in any::<bool>()) {
        keys.sort();
        let mut vertical=BTreeMap::new(); let mut horizontal=BTreeMap::new();
        for (position,key) in keys.iter().enumerate() {
            vertical.insert((position as u32,0),Value::Number(f64::from(*key)));
            vertical.insert((position as u32,1),Value::Number((position+101) as f64));
            horizontal.insert((0,position as u32),Value::Number(f64::from(*key)));
            horizontal.insert((1,position as u32),Value::Number((position+101) as f64));
        }
        let mode=if approximate {"TRUE"} else {"FALSE"};
        let v=format!("=VLOOKUP({lookup},A1:B{},2,{mode})",keys.len());
        let end=char::from(b'A'+(keys.len()-1) as u8);
        let h=format!("=HLOOKUP({lookup},A1:{end}2,2,{mode})");
        let position=if approximate {keys.iter().rposition(|k|*k<=lookup)} else {keys.iter().position(|k|*k==lookup)};
        let expected=position.map_or(Value::Error(FormulaError::Na),|p|Value::Number((p+101) as f64));
        let actual=value(&v,&Table(vertical)).map_err(|e|TestCaseError::fail(e.to_string()))?;
        let transposed=value(&h,&Table(horizontal)).map_err(|e|TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(&actual,&expected); prop_assert_eq!(actual,transposed);
    }
}

#[test]
fn visits_only_key_axis_and_selected_return_cell() -> Result<(), Box<dyn std::error::Error>> {
    use std::cell::RefCell;
    struct Observed {
        ranges: RefCell<Vec<(u32, u32, u32, u32)>>,
        cells: RefCell<Vec<(u32, u32)>>,
    }
    impl ReferenceResolver for Observed {
        fn resolve_cell(&self, cell: &CellReference) -> Value {
            self.cells.borrow_mut().push((cell.row, cell.column));
            Value::Number(42.)
        }
        fn visit_range(
            &self,
            range: &RangeReference,
            visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            self.ranges.borrow_mut().push((
                range.start.row,
                range.start.column,
                range.end.row,
                range.end.column,
            ));
            for _ in 0..range.cell_count() {
                visitor(Value::Number(7.))?;
            }
            Ok(())
        }
    }
    for (source, axis, selected) in [
        ("=VLOOKUP(7,B2:D4,3,FALSE)", (1, 1, 3, 1), (1, 3)),
        ("=HLOOKUP(7,B2:D4,3,FALSE)", (1, 1, 1, 3), (3, 1)),
    ] {
        let resolver = Observed {
            ranges: RefCell::new(vec![]),
            cells: RefCell::new(vec![]),
        };
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &resolver,
                &mut EvaluationBudget::default()
            )?,
            Value::Number(42.)
        );
        assert_eq!(*resolver.ranges.borrow(), vec![axis]);
        assert_eq!(*resolver.cells.borrow(), vec![selected]);
    }
    struct Broken(usize);
    impl ReferenceResolver for Broken {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Number(7.)
        }
        fn visit_range(
            &self,
            _: &RangeReference,
            visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            for _ in 0..self.0 {
                visitor(Value::Number(7.))?;
            }
            Ok(())
        }
    }
    for visits in [1, 3] {
        assert_eq!(
            evaluate(
                &parse("=IFERROR(VLOOKUP(7,A1:B2,2,FALSE),99)")?.ast,
                &Broken(visits),
                &mut EvaluationBudget::default()
            ),
            Err(EvaluationError::InvalidResolverResult)
        );
    }
    Ok(())
}

#[test]
fn argument_boundaries_datetime_and_runtime_table_cap() -> Result<(), Box<dyn std::error::Error>> {
    for (argument, expected) in [
        (
            Value::Text(" ".repeat(40000)),
            EvaluationError::CalculationLimitExceeded,
        ),
        (
            Value::Array {
                rows: 1,
                columns: 1,
                values: vec![Value::Number(1.)],
            },
            EvaluationError::UnsupportedCompatibilityFeature,
        ),
    ] {
        let table = Table(BTreeMap::from([
            ((0, 0), Value::Number(7.)),
            ((0, 1), Value::Number(21.)),
            ((0, 2), argument),
        ]));
        for source in [
            "=IFERROR(VLOOKUP(7,A1:B1,C1,FALSE),99)",
            "=IFERROR(HLOOKUP(7,A1:B1,1,C1),99)",
        ] {
            assert_eq!(
                evaluate(
                    &parse(source)?.ast,
                    &table,
                    &mut EvaluationBudget::default()
                ),
                Err(expected)
            );
        }
    }
    for serial in [0., -0., 45123.5, f64::INFINITY, f64::NAN] {
        let table = Table(BTreeMap::from([
            ((0, 0), Value::Number(7.)),
            (
                (0, 1),
                Value::DateTime {
                    serial,
                    system: DateSystem::Excel1904,
                },
            ),
        ]));
        let result = value("=VLOOKUP(7,A1:B1,2,FALSE)", &table)?;
        if serial.is_finite() {
            assert_eq!(
                result,
                Value::DateTime {
                    serial: if serial == 0. { 0. } else { serial },
                    system: DateSystem::Excel1904
                }
            );
            if let Value::DateTime { serial, .. } = result
                && serial == 0.
            {
                assert_eq!(serial.to_bits(), 0.0f64.to_bits());
            }
        } else {
            assert_eq!(result, Value::Error(FormulaError::Num));
        }
    }
    let mut ast = parse("=IFERROR(VLOOKUP(7,A1:B2,1,FALSE),99)")?.ast;
    for node in &mut ast.nodes {
        if let Node::Range(range) = node {
            range.end.row = 50000;
        }
    }
    assert_eq!(
        evaluate(
            &ast,
            &Table(BTreeMap::new()),
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let table = Table(BTreeMap::from([
        ((1, 1), Value::Text("Alpha".into())),
        ((2, 1), Value::Boolean(true)),
    ]));
    assert_eq!(
        value("=HLOOKUP(\"a*\",INDEX(B2:C3,0,0),2,FALSE)", &table)?,
        Value::Boolean(true)
    );
    assert_eq!(
        value("=COUNT(HLOOKUP(\"a*\",B2:C3,2,FALSE))", &table)?,
        Value::Number(1.)
    );
    assert_eq!(
        value("=COUNTA(HLOOKUP(\"a*\",B2:C3,2,FALSE))", &table)?,
        Value::Number(1.)
    );
    Ok(())
}
#[test]
fn vertical_horizontal_exact_approximate_and_index_table() -> Result<(), Box<dyn std::error::Error>>
{
    let table = Table(BTreeMap::from([
        ((0, 0), Value::Number(5.)),
        ((1, 0), Value::Number(25.)),
        ((2, 0), Value::Number(38.)),
        ((0, 1), Value::Text("first".into())),
        ((1, 1), Value::Text("second".into())),
        ((2, 1), Value::Text("third".into())),
        ((0, 2), Value::Number(13.)),
        ((1, 2), Value::Number(21.)),
        ((2, 2), Value::Number(29.)),
    ]));
    for (source, expected) in [
        ("=VLOOKUP(25,A1:C3,2,FALSE)", Value::Text("second".into())),
        ("=VLOOKUP(26,A1:C3,3)", Value::Number(21.)),
        ("=HLOOKUP(13,A1:C3,2,FALSE)", Value::Number(21.)),
        (
            "=VLOOKUP(25,INDEX(A1:C3,0,0),2,FALSE)",
            Value::Text("second".into()),
        ),
        ("=VLOOKUP(4,A1:C3,2,TRUE)", Value::Error(FormulaError::Na)),
        (
            "=VLOOKUP(25,A1:C3,4,FALSE)",
            Value::Error(FormulaError::Ref),
        ),
        (
            "=HLOOKUP(13,A1:C3,0,FALSE)",
            Value::Error(FormulaError::Value),
        ),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &table,
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    Ok(())
}
