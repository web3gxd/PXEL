use proptest::prelude::*;
use spreadsheet_formula::*;
use std::collections::BTreeMap;
struct Cells(BTreeMap<(u32, u32), Value>);

#[test]
fn multiple_criteria_boolean_sums_errors_and_pair_limit() -> Result<(), Box<dyn std::error::Error>>
{
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(1.)),
        ((1, 0), Value::Number(2.)),
        ((2, 0), Value::Number(3.)),
        ((0, 1), Value::Boolean(true)),
        ((1, 1), Value::Boolean(false)),
        ((2, 1), Value::Error(FormulaError::Div0)),
    ]));
    for (source, expected) in [
        ("=SUMIFS(B1:B3,A1:A3,\"<3\")", Value::Number(1.)),
        ("=SUMIFS(B1:B3,A1:A3,3)", Value::Error(FormulaError::Div0)),
        ("=SUMIFS(B1:B3,A1:A3,99)", Value::Number(0.)),
        ("=SUMIF(A1:A3,\"<3\",B1)", Value::Number(0.)),
        (
            "=COUNTIFS(A1:A3,\">0\",B1:B3,\"#DIV/0!\")",
            Value::Number(1.),
        ),
        (
            "=COUNTIFS(A1:A3,99,B1:B3,1/0)",
            Value::Error(FormulaError::Div0),
        ),
        ("=COUNTIFS(A1:A3,C1)", Value::Number(0.)),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            ),
            Ok(expected),
            "{source}"
        );
    }
    for function in ["COUNTIFS", "SUMIFS"] {
        let prefix = if function == "SUMIFS" { "A1," } else { "" };
        let source = format!("={function}({prefix}{})", vec!["A1,1"; 127].join(","));
        assert_eq!(
            evaluate(
                &parse(&source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            )?,
            Value::Number(1.)
        );
        let source = format!("={function}({prefix}{})", vec!["A1,1"; 128].join(","));
        assert!(parse(&source).is_err());
    }
    Ok(())
}

#[test]
fn multiple_criteria_never_skip_typed_boundaries_after_no_match()
-> Result<(), Box<dyn std::error::Error>> {
    for (value, expected) in [
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
        let cells = Cells(BTreeMap::from([
            ((0, 0), Value::Number(1.)),
            ((0, 1), value),
        ]));
        for source in [
            "=IFERROR(COUNTIFS(A1,99,B1,1),42)",
            "=IFERROR(SUMIFS(B1,A1,99),42)",
        ] {
            assert_eq!(
                evaluate(
                    &parse(source)?.ast,
                    &cells,
                    &mut EvaluationBudget::default()
                ),
                Err(expected),
                "{source}"
            );
        }
    }
    struct BrokenColumn {
        column: u32,
        count: usize,
    }
    impl ReferenceResolver for BrokenColumn {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Number(1.)
        }
        fn visit_range(
            &self,
            range: &RangeReference,
            visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            let count = if range.start.column == self.column {
                self.count
            } else {
                2
            };
            for _ in 0..count {
                visitor(Value::Number(1.))?;
            }
            Ok(())
        }
    }
    for (source, column) in [
        ("=COUNTIFS(A1:A2,99,B1:B2,1)", 1),
        ("=SUMIFS(C1:C2,A1:A2,99,B1:B2,1)", 2),
    ] {
        let ast = parse(source)?.ast;
        for count in [1, 3] {
            assert_eq!(
                evaluate(
                    &ast,
                    &BrokenColumn { column, count },
                    &mut EvaluationBudget::default()
                ),
                Err(EvaluationError::InvalidResolverResult)
            );
        }
        assert_eq!(
            evaluate(
                &ast,
                &BrokenColumn { column, count: 2 },
                &mut EvaluationBudget::new(10)
            ),
            Err(EvaluationError::CalculationLimitExceeded)
        );
        let mut cancelled = EvaluationBudget::default();
        cancelled.cancel();
        assert_eq!(
            evaluate(&ast, &BrokenColumn { column, count: 2 }, &mut cancelled),
            Err(EvaluationError::Cancelled)
        );
    }
    Ok(())
}

proptest! {
    #[test]
    fn multiple_criteria_match_independent_intersection_oracle(values in prop::collection::vec((-100i16..100,-100i16..100),1..30), threshold in -100i16..100) {
        let mut map=BTreeMap::new();
        for (row,(a,b)) in values.iter().enumerate(){ map.insert((row as u32,0),Value::Number(f64::from(*a))); map.insert((row as u32,1),Value::Number(f64::from(*b))); }
        let cells=Cells(map);
        for (source,expected) in [
            (format!("=COUNTIFS(A1:A{},\">{threshold}\",B1:B{},\"<0\")",values.len(),values.len()),values.iter().filter(|(a,b)|*a>threshold&&*b<0).count() as f64),
            (format!("=SUMIFS(B1:B{},A1:A{},\">{threshold}\",B1:B{},\"<0\")",values.len(),values.len(),values.len()),values.iter().filter(|(a,b)|*a>threshold&&*b<0).map(|(_,b)|f64::from(*b)).sum()),
        ] {
            let ast=parse(&source).map_err(|e|TestCaseError::fail(e.to_string()))?.ast;
            let actual=evaluate(&ast,&cells,&mut EvaluationBudget::default()).map_err(|e|TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(actual,Value::Number(expected));
        }
    }
}

#[test]
fn multiple_criteria_intersect_and_require_identical_shape()
-> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(5.)),
        ((1, 0), Value::Number(25.)),
        ((2, 0), Value::Number(38.)),
        ((0, 1), Value::Text("Apple".into())),
        ((1, 1), Value::Text("apple*".into())),
        ((2, 1), Value::Text("Pear".into())),
        ((0, 2), Value::Number(10.)),
        ((1, 2), Value::Number(20.)),
        ((2, 2), Value::Number(30.)),
    ]));
    for (source, expected) in [
        (
            "=COUNTIFS(A1:A3,\">5\",B1:B3,\"apple*\")",
            Value::Number(1.),
        ),
        (
            "=SUMIFS(C1:C3,A1:A3,\">5\",B1:B3,\"apple*\")",
            Value::Number(20.),
        ),
        ("=COUNTIFS(A1:A3,\">5\",A3:A1,\"<30\")", Value::Number(1.)),
        ("=SUMIFS(INDEX(C1:C3,0),A1:A3,\">5\")", Value::Number(50.)),
        (
            "=COUNTIFS(A1:A3,\">5\",B1,\"*\")",
            Value::Error(FormulaError::Value),
        ),
        (
            "=COUNTIFS(A1:A2,\">0\",A1:B1,\">0\")",
            Value::Error(FormulaError::Value),
        ),
        (
            "=SUMIFS(C1,A1:A3,\">5\")",
            Value::Error(FormulaError::Value),
        ),
        ("=COUNTIFS()", Value::Error(FormulaError::Value)),
        ("=COUNTIFS(A1:A3)", Value::Error(FormulaError::Value)),
        ("=SUMIFS(C1:C3,A1:A3)", Value::Error(FormulaError::Value)),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            ),
            Ok(expected),
            "{source}"
        );
    }
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
fn rectangular_reversed_and_dynamic_ranges_keep_row_major_alignment()
-> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(1.)),
        ((0, 1), Value::Number(2.)),
        ((1, 0), Value::Number(3.)),
        ((1, 1), Value::Number(4.)),
        ((0, 2), Value::Number(10.)),
        ((0, 3), Value::Number(20.)),
        ((1, 2), Value::Number(30.)),
        ((1, 3), Value::Number(40.)),
    ]));
    for (source, expected) in [
        ("=SUMIF(A1:B2,\">2\",C1)", 70.),
        ("=SUMIF(B2:A1,\">2\",D2:C1)", 70.),
        ("=COUNTIF(B2:A1,\">=2\")", 3.),
        ("=SUMIF(A1:A2,\">0\",INDEX(C1:D2,0,0))", 40.),
        ("=SUMIF(A1:A2,\">0\",XLOOKUP(1,A1:B1,C1:D2))", 40.),
        ("=COUNTIF(INDEX(A1:B2,0,2),\">2\")", 1.),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            ),
            Ok(Value::Number(expected)),
            "{source}"
        );
    }
    Ok(())
}

#[test]
fn separate_sum_range_requires_exact_visitor_cardinality() -> Result<(), Box<dyn std::error::Error>>
{
    struct BrokenSum(usize);
    impl ReferenceResolver for BrokenSum {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Number(1.)
        }
        fn visit_range(
            &self,
            range: &RangeReference,
            visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            let count = if range.start.column == 0 { 2 } else { self.0 };
            for _ in 0..count {
                visitor(Value::Number(1.))?;
            }
            Ok(())
        }
    }
    let ast = parse("=SUMIF(A1:A2,1,B1)")?.ast;
    for count in [1, 3] {
        assert_eq!(
            evaluate(&ast, &BrokenSum(count), &mut EvaluationBudget::default()),
            Err(EvaluationError::InvalidResolverResult)
        );
    }
    Ok(())
}

#[test]
fn text_blank_boolean_and_error_criteria() -> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Text("Apple".into())),
        ((1, 0), Value::Text("apple*".into())),
        ((2, 0), Value::Blank),
        ((3, 0), Value::Text(String::new())),
        ((4, 0), Value::Number(0.)),
        ((5, 0), Value::Text("0".into())),
        ((6, 0), Value::Boolean(true)),
        ((7, 0), Value::Error(FormulaError::Div0)),
        ((0, 1), Value::Number(10.)),
        ((1, 1), Value::Number(20.)),
        ((2, 1), Value::Number(30.)),
    ]));
    for (source, expected) in [
        ("=COUNTIF(A1:A8,\"apple*\")", Value::Number(2.)),
        ("=COUNTIF(A1:A8,\"apple~*\")", Value::Number(1.)),
        ("=COUNTIF(A1:A8,\"\")", Value::Number(2.)),
        ("=COUNTIF(A1:A8,\"<>\")", Value::Number(7.)),
        ("=COUNTIF(A1:A8,0)", Value::Number(3.)),
        ("=COUNTIF(A1:A8,C1)", Value::Number(3.)),
        ("=COUNTIF(A1:A8,TRUE)", Value::Number(1.)),
        ("=COUNTIF(A1:A8,\"#DIV/0!\")", Value::Number(1.)),
        ("=COUNTIF(A1:A8,1/0)", Value::Error(FormulaError::Div0)),
        ("=COUNTIF(A1:A8,\"*\")", Value::Number(3.)),
        ("=SUMIF(A1:A3,\"apple*\",B1)", Value::Number(30.)),
        ("=SUMIF(A1:A3,\"\",B1)", Value::Number(30.)),
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
fn verified_excel_criteria_profile_2026_10_02() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/formulas/excel-criteria-2026-10-02.json"
    ))?;
    assert_eq!(fixture["verified_in_excel"], true);
    let mut values = BTreeMap::new();
    for cell in fixture["workbook"]["cells"]
        .as_array()
        .ok_or("Excel golden workbook cells missing")?
    {
        let row = u32::try_from(cell["row"].as_u64().ok_or("cell row missing")?)? - 1;
        let column = u32::try_from(cell["column"].as_u64().ok_or("cell column missing")?)? - 1;
        let value = match cell["type"].as_str().ok_or("cell type missing")? {
            "number" => Value::Number(cell["value"].as_f64().ok_or("number missing")?),
            "text" => Value::Text(
                cell["value"]
                    .as_str()
                    .ok_or("text value missing")?
                    .to_owned(),
            ),
            "boolean" => Value::Boolean(cell["value"].as_bool().ok_or("boolean missing")?),
            "error" if cell["value"] == "#DIV/0!" => Value::Error(FormulaError::Div0),
            "blank" => Value::Blank,
            _ => return Err("unsupported Excel fixture cell type".into()),
        };
        values.insert((row, column), value);
    }
    let cells = Cells(values);
    for case in fixture["cases"]
        .as_array()
        .ok_or("Excel golden formula cases missing")?
    {
        let source = case["formula"].as_str().ok_or("formula missing")?;
        let expected = case["result"].as_f64().ok_or("result missing")?;
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            )?,
            Value::Number(expected),
            "Excel 16.0.20326.20158: {source}"
        );
    }
    Ok(())
}

#[test]
fn selected_sum_errors_overflow_and_dynamic_extent_are_controlled()
-> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(7.)),
        ((1, 0), Value::Number(13.)),
        ((0, 1), Value::Number(21.)),
        ((1, 1), Value::Error(FormulaError::Div0)),
    ]));
    for (source, expected) in [
        ("=SUMIF(A1:A2,7,B1)", Value::Number(21.)),
        ("=SUMIF(A1:A2,13,B1)", Value::Error(FormulaError::Div0)),
        ("=SUMIF(A1:A2,99,B1)", Value::Number(0.)),
        (
            "=SUMIF(A1:A2,\">0\",INDEX(B1:B2,0))",
            Value::Error(FormulaError::Div0),
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
    assert_eq!(
        evaluate(
            &parse("=IFERROR(SUMIF(INDEX(A1:A2,0),7,B1),99)")?.ast,
            &cells,
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::UnsupportedCompatibilityFeature)
    );
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(1e308)),
        ((1, 0), Value::Number(1e308)),
    ]));
    assert_eq!(
        evaluate(
            &parse("=SUMIF(A1:A2,\">0\")")?.ast,
            &cells,
            &mut EvaluationBudget::default()
        )?,
        Value::Error(FormulaError::Num)
    );
    Ok(())
}

#[test]
fn criteria_and_all_visited_values_preserve_typed_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    for (value, expected) in [
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
        let cells = Cells(BTreeMap::from([
            ((0, 0), Value::Number(7.)),
            ((1, 0), value.clone()),
            ((0, 1), value),
        ]));
        for source in [
            "=IFERROR(COUNTIF(A1:A2,99),42)",
            "=IFERROR(COUNTIF(A1,B1),42)",
            "=IFERROR(SUMIF(A1,99,B1),42)",
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
    let cells = Cells(BTreeMap::from([((0, 0), Value::Text("x".repeat(256)))]));
    assert_eq!(
        evaluate(
            &parse("=IFERROR(COUNTIF(B1,A1),99)")?.ast,
            &cells,
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::UnsupportedCompatibilityFeature)
    );
    let ast = parse("=COUNTIF(A1:A2,7)")?.ast;
    let mut budget = EvaluationBudget::default();
    budget.cancel();
    assert_eq!(
        evaluate(&ast, &cells, &mut budget),
        Err(EvaluationError::Cancelled)
    );
    assert_eq!(
        evaluate(&ast, &cells, &mut EvaluationBudget::new(3)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
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
            evaluate(&ast, &Broken(visits), &mut EvaluationBudget::default()),
            Err(EvaluationError::InvalidResolverResult)
        );
    }
    Ok(())
}

proptest! {
    #[test]
    fn numeric_threshold_matches_count_and_sum_oracle(values in prop::collection::vec(-100i16..100,1..30),threshold in -110i16..110) {
        let cells=Cells(values.iter().enumerate().map(|(row,n)|((row as u32,0),Value::Number(f64::from(*n)))).collect());
        for (function,expected) in [("COUNTIF",values.iter().filter(|n|**n>threshold).count() as f64),("SUMIF",values.iter().filter(|n|**n>threshold).map(|n|f64::from(*n)).sum())] {
            let ast=parse(&format!("={function}(A1:A{},\">{threshold}\")",values.len())).map_err(|e|TestCaseError::fail(e.to_string()))?.ast;
            let result=evaluate(&ast,&cells,&mut EvaluationBudget::default()).map_err(|e|TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(result,Value::Number(expected));
        }
    }
}
#[test]
fn numeric_criteria_and_top_left_sum_range() -> Result<(), Box<dyn std::error::Error>> {
    let cells = Cells(BTreeMap::from([
        ((0, 0), Value::Number(5.)),
        ((1, 0), Value::Number(25.)),
        ((2, 0), Value::Number(38.)),
        ((0, 1), Value::Number(10.)),
        ((1, 1), Value::Number(20.)),
        ((2, 1), Value::Number(30.)),
    ]));
    for (source, expected) in [
        ("=COUNTIF(A1:A3,\">5\")", 2.),
        ("=SUMIF(A1:A3,\">5\")", 63.),
        ("=SUMIF(A1:A3,\">5\",B1:B3)", 50.),
        ("=SUMIF(A1:A3,\">5\",B1)", 50.),
        ("=SUMIF(INDEX(A1:A3,0),\">5\",B1:B3)", 50.),
        ("=COUNTIF(A1:A3,25)", 1.),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &cells,
                &mut EvaluationBudget::default()
            )?,
            Value::Number(expected),
            "{source}"
        );
    }
    Ok(())
}
