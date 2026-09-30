use proptest::prelude::*;
use spreadsheet_formula::*;
use std::collections::BTreeMap;
struct Cells(BTreeMap<(u32, u32), Value>);

#[test]
fn runtime_source_return_caps_and_wildcard_work_are_typed() -> Result<(), Box<dyn std::error::Error>>
{
    let cells = Cells(BTreeMap::from([((0, 0), Value::Number(7.))]));
    for return_range in [false, true] {
        let mut ast = parse("=IFERROR(XLOOKUP(7,A1:A2,B1:C2,99),42)")?.ast;
        for node in &mut ast.nodes {
            if let Node::Range(range) = node
                && (range.start.column == 1) == return_range
            {
                range.end.row = if return_range { 50000 } else { 100000 };
            }
        }
        assert_eq!(
            evaluate(&ast, &cells, &mut EvaluationBudget::default()),
            Err(EvaluationError::CalculationLimitExceeded)
        );
    }
    let ast = parse(&format!(
        "=IFERROR(XLOOKUP(\"{}\",A1,B1,99,2),42)",
        "*a".repeat(16)
    ))?
    .ast;
    let cells = Cells(BTreeMap::from([((0, 0), Value::Text("a".repeat(200)))]));
    assert_eq!(
        evaluate(&ast, &cells, &mut EvaluationBudget::new(1000)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}
impl ReferenceResolver for Cells {
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
        for row in range.start.row..=range.end.row {
            for column in range.start.column..=range.end.column {
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
fn blank_wildcard_horizontal_and_nested_references() -> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Text("Alpha".into())),
        ((1, 0), Value::Blank),
        ((2, 0), Value::Number(0.)),
        ((0, 1), Value::Boolean(true)),
        ((1, 1), Value::Number(21.)),
        ((2, 1), Value::Number(31.)),
        ((0, 2), Value::Number(7.)),
        ((1, 2), Value::Number(13.)),
    ]));
    for (source, expected) in [
        ("=XLOOKUP(,A1:A3,B1:B3)", Value::Number(21.)),
        ("=XLOOKUP(0,A1:A3,B1:B3)", Value::Number(31.)),
        ("=XLOOKUP(\"a*\",A1:A3,B1:B3,99)", Value::Number(99.)),
        ("=XLOOKUP(\"a*\",A1:A3,B1:B3,99,2)", Value::Boolean(true)),
        ("=COUNT(XLOOKUP(\"Alpha\",A1:A3,B1:B3))", Value::Number(0.)),
        ("=COUNTA(XLOOKUP(\"Alpha\",A1:A3,B1:B3))", Value::Number(1.)),
        ("=SUM(XLOOKUP(7,C1:C2,A1:B2))", Value::Number(0.)),
        ("=SUM(XLOOKUP(7,A1:C1,A2:C2))", Value::Number(13.)),
        (
            "=XLOOKUP(21,A2:C2,XLOOKUP(13,C1:C2,A1:C2))",
            Value::Number(21.),
        ),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    Ok(())
}

#[test]
fn omitted_fallback_is_na_and_selected_text_limits_cover_aggregates()
-> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(7.)),
        ((0, 1), Value::Text("x".repeat(40000))),
    ]));
    for source in ["=XLOOKUP(13,A1,B1,,0,-1)", "=SUM(XLOOKUP(13,A1,B1,,0,-1))"] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            )?,
            Value::Error(FormulaError::Na)
        );
    }
    assert_eq!(
        evaluate(
            &parse("=XLOOKUP(13,A1,B1,C1,0,-1)")?.ast,
            &cells,
            &mut EvaluationBudget::default()
        )?,
        Value::Number(0.)
    );
    for source in [
        "=IFERROR(INDEX(XLOOKUP(7,A1,B1),1),99)",
        "=IFERROR(COUNT(XLOOKUP(7,A1,B1)),99)",
        "=IFERROR(SUM(XLOOKUP(7,A1,B1)),99)",
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            ),
            Err(EvaluationError::CalculationLimitExceeded)
        );
    }
    Ok(())
}

#[test]
fn nested_index_dates_share_scalar_normalization_and_text_budget()
-> Result<(), Box<dyn std::error::Error>> {
    for serial in [-0., 45123.5, f64::INFINITY] {
        let cells = Cells(BTreeMap::from([
            ((0, 0), Value::Number(7.)),
            (
                (0, 1),
                Value::DateTime {
                    serial,
                    system: DateSystem::Excel1904,
                },
            ),
        ]));
        let actual = evaluate(
            &parse("=INDEX(XLOOKUP(7,A1,B1),1)")?.ast,
            &cells,
            &mut EvaluationBudget::default(),
        )?;
        if serial.is_finite() {
            assert_eq!(
                actual,
                Value::DateTime {
                    serial: if serial == 0. { 0. } else { serial },
                    system: DateSystem::Excel1904
                }
            );
            if let Value::DateTime { serial, .. } = actual
                && serial == 0.
            {
                assert_eq!(serial.to_bits(), 0.0f64.to_bits());
            }
        } else {
            assert_eq!(actual, Value::Error(FormulaError::Num));
        }
    }
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(7.)),
        ((0, 1), Value::Text("x".repeat(100))),
    ]));
    assert_eq!(
        evaluate(
            &parse("=INDEX(XLOOKUP(7,A1,B1),1)")?.ast,
            &cells,
            &mut EvaluationBudget::new(40)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}

#[test]
fn error_precedence_shape_and_lazy_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(7.)),
        ((1, 0), Value::Error(FormulaError::Div0)),
        ((0, 1), Value::Blank),
    ]));
    for (source, expected) in [
        ("=XLOOKUP(7,A1:A2,B1:B2,99)", Value::Number(0.)),
        (
            "=XLOOKUP(7,A1:A2,B1:B2,99,0,-1)",
            Value::Error(FormulaError::Div0),
        ),
        (
            "=XLOOKUP(13,A1:A2,B1:B2,99)",
            Value::Error(FormulaError::Div0),
        ),
        ("=XLOOKUP(7,A1:A2,B1,99)", Value::Error(FormulaError::Value)),
        (
            "=XLOOKUP(7,A1:B2,B1:B2,99)",
            Value::Error(FormulaError::Value),
        ),
        (
            "=XLOOKUP(7,A1:A2,B1:B2,99,3)",
            Value::Error(FormulaError::Value),
        ),
        (
            "=XLOOKUP(7,A1:A2,B1:B2,99,0,0)",
            Value::Error(FormulaError::Value),
        ),
        ("=XLOOKUP(7,A1:A2,B1:B2,1/0)", Value::Number(0.)),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    assert_eq!(
        evaluate(
            &parse("=XLOOKUP(7,A1:A2,B1:B2,99,2,2)")?.ast,
            &cells,
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::UnsupportedCompatibilityFeature)
    );
    Ok(())
}

#[test]
fn all_key_boundaries_and_visitor_cardinality_survive_iferror()
-> Result<(), Box<dyn std::error::Error>> {
    for (bad, expected) in [
        (
            Value::Array {
                rows: 1,
                columns: 1,
                values: vec![Value::Number(1.)],
            },
            EvaluationError::UnsupportedCompatibilityFeature,
        ),
        (
            Value::Text("x".repeat(40000)),
            EvaluationError::CalculationLimitExceeded,
        ),
    ] {
        let cells = Cells(BTreeMap::from([
            ((0, 0), Value::Number(7.)),
            ((1, 0), bad.clone()),
            ((0, 1), Value::Number(21.)),
            ((0, 2), bad),
        ]));
        for source in [
            "=IFERROR(XLOOKUP(7,A1:A2,B1:B2,99),42)",
            "=IFERROR(XLOOKUP(7,A1,B1,99,C1),42)",
            "=IFERROR(XLOOKUP(7,A1,B1,99,0,C1),42)",
        ] {
            assert_eq!(
                evaluate(
                    &parse(source)?.ast,
                    &cells,
                    &mut EvaluationBudget::default()
                ),
                Err(expected)
            );
        }
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
    let ast = parse("=IFERROR(XLOOKUP(7,A1:A2,B1:B2,99),42)")?.ast;
    for count in [1, 3] {
        assert_eq!(
            evaluate(&ast, &Broken(count), &mut EvaluationBudget::default()),
            Err(EvaluationError::InvalidResolverResult)
        );
    }
    let mut cancelled = EvaluationBudget::default();
    cancelled.cancel();
    assert_eq!(
        evaluate(&ast, &Broken(2), &mut cancelled),
        Err(EvaluationError::Cancelled)
    );
    assert_eq!(
        evaluate(&ast, &Broken(2), &mut EvaluationBudget::new(3)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}

proptest! {
    #[test]
    fn binary_neighbors_match_value_oracle(mut keys in prop::collection::vec(-20i16..20,1..30),lookup in -25i16..25,descending in any::<bool>(),mode in -1i8..2) {
        keys.sort(); if descending {keys.reverse();}
        let data=keys.iter().enumerate().flat_map(|(row,key)| [((row as u32,0),Value::Number(f64::from(*key))),((row as u32,1),Value::Number(f64::from(*key)*2.))]).collect();
        let exact=keys.iter().copied().find(|k|*k==lookup);
        let closest=if mode==-1 {keys.iter().copied().filter(|n|*n<lookup).max()} else if mode==1 {keys.iter().copied().filter(|n|*n>lookup).min()} else {None};
        let expected=exact.or(closest).map_or(Value::Number(99.),|key|Value::Number(f64::from(key)*2.));
        let source=format!("=XLOOKUP({lookup},A1:A{},B1:B{},99,{mode},{})",keys.len(),keys.len(),if descending {-2} else {2});
        let ast=parse(&source).map_err(|e|TestCaseError::fail(e.to_string()))?.ast;
        let result=evaluate(&ast,&Cells(data),&mut EvaluationBudget::default()).map_err(|e|TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(result,expected);
    }
    #[test]
    fn unsorted_linear_nearest_matches_independent_oracle(keys in prop::collection::vec(-20i16..20,1..30),lookup in -25i16..25,reverse in any::<bool>(),mode in -1i8..2) {
        let data=keys.iter().enumerate().flat_map(|(row,key)| [((row as u32,0),Value::Number(f64::from(*key))),((row as u32,1),Value::Number((row+101) as f64))]).collect();
        let mut indices=(0..keys.len()).collect::<Vec<_>>(); if reverse {indices.reverse();}
        let exact=indices.iter().copied().find(|p|keys[*p]==lookup);
        let closest=if mode==-1 {keys.iter().copied().filter(|n|*n<lookup).max()} else if mode==1 {keys.iter().copied().filter(|n|*n>lookup).min()} else {None};
        let position=exact.or_else(||closest.and_then(|key|indices.iter().copied().find(|p|keys[*p]==key)));
        let expected=position.map_or(Value::Number(99.),|p|Value::Number((p+101) as f64));
        let source=format!("=XLOOKUP({lookup},A1:A{},B1:B{},99,{mode},{})",keys.len(),keys.len(),if reverse {-1} else {1});
        let ast=parse(&source).map_err(|e|TestCaseError::fail(e.to_string()))?.ast;
        let result=evaluate(&ast,&Cells(data),&mut EvaluationBudget::default()).map_err(|e|TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(result,expected);
    }
}

#[test]
fn binary_modes_validate_order_and_select_neighbors() -> Result<(), Box<dyn std::error::Error>> {
    for (keys, direction, exact, floor) in [
        (vec![5., 25., 25., 38.], 2, 20., 30.),
        (vec![38., 25., 25., 5.], -2, 20., 20.),
    ] {
        let cells = Cells(
            keys.into_iter()
                .enumerate()
                .flat_map(|(row, key)| {
                    [
                        ((row as u32, 0), Value::Number(key)),
                        ((row as u32, 1), Value::Number((row + 1) as f64 * 10.)),
                    ]
                })
                .collect(),
        );
        for (lookup, mode, expected) in [(25, 0, exact), (26, -1, floor)] {
            let source = format!("=XLOOKUP({lookup},A1:A4,B1:B4,99,{mode},{direction})");
            assert_eq!(
                evaluate(
                    &parse(&source)?.ast,
                    &cells,
                    &mut EvaluationBudget::default()
                )?,
                Value::Number(expected)
            );
        }
    }
    for (keys, direction) in [(vec![5., 25., 38.], 2), (vec![38., 25., 5.], -2)] {
        let mut data = BTreeMap::new();
        for (row, key) in keys.iter().enumerate() {
            data.insert((row as u32, 0), Value::Number(*key));
            data.insert((row as u32, 1), Value::Number(*key * 2.));
        }
        let cells = Cells(data);
        for (lookup, mode, expected) in [
            (25, 0, 50.),
            (26, -1, 50.),
            (26, 1, 76.),
            (4, -1, 99.),
            (50, 1, 99.),
        ] {
            let source = format!("=XLOOKUP({lookup},A1:A3,B1:B3,99,{mode},{direction})");
            assert_eq!(
                evaluate(
                    &parse(&source)?.ast,
                    &cells,
                    &mut EvaluationBudget::default()
                )?,
                Value::Number(expected),
                "{source}"
            );
        }
    }
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(25.)),
        ((1, 0), Value::Number(5.)),
        ((2, 0), Value::Number(38.)),
    ]));
    assert_eq!(
        evaluate(
            &parse("=XLOOKUP(25,A1:A3,B1:B3,99,0,2)")?.ast,
            &cells,
            &mut EvaluationBudget::default()
        )?,
        Value::Error(FormulaError::Na)
    );
    Ok(())
}
#[test]
fn exact_nearest_direction_and_reference_return() -> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(5.)),
        ((1, 0), Value::Number(25.)),
        ((2, 0), Value::Number(25.)),
        ((3, 0), Value::Number(38.)),
        ((0, 1), Value::Number(10.)),
        ((1, 1), Value::Number(20.)),
        ((2, 1), Value::Number(30.)),
        ((3, 1), Value::Number(40.)),
        ((1, 2), Value::Number(2.)),
    ]));
    for (source, expected) in [
        ("=XLOOKUP(25,A1:A4,B1:B4)", Value::Number(20.)),
        ("=XLOOKUP(25,A1:A4,B1:B4,,0,-1)", Value::Number(30.)),
        ("=XLOOKUP(26,A1:A4,B1:B4,99,-1)", Value::Number(20.)),
        ("=XLOOKUP(26,A1:A4,B1:B4,99,1)", Value::Number(40.)),
        ("=XLOOKUP(4,A1:A4,B1:B4,99,-1)", Value::Number(99.)),
        ("=SUM(XLOOKUP(25,A1:A4,B1:C4))", Value::Number(22.)),
        ("=INDEX(XLOOKUP(25,A1:A4,B1:C4),1,2)", Value::Number(2.)),
        ("=XLOOKUP(25,A1:A4,B1:B4,1/0)", Value::Number(20.)),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    Ok(())
}
