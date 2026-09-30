use proptest::prelude::*;
use spreadsheet_formula::*;

struct Vector(Vec<Value>);
impl ReferenceResolver for Vector {
    fn resolve_cell(&self, cell: &CellReference) -> Value {
        self.0
            .get(cell.row as usize)
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
fn match_exact_approximate_and_reference_sources() -> Result<(), Box<dyn std::error::Error>> {
    for (source, values, expected) in [
        ("=MATCH(25,A1:A3,0)", vec![5., 25., 38.], Value::Number(2.)),
        ("=MATCH(26,A1:A3)", vec![5., 25., 38.], Value::Number(2.)),
        ("=MATCH(26,A1:A3,-1)", vec![38., 25., 5.], Value::Number(1.)),
        ("=MATCH(25,A1:A3,0)", vec![25., 25., 38.], Value::Number(1.)),
        ("=MATCH(25,A1:A3,1)", vec![25., 25., 38.], Value::Number(2.)),
        (
            "=MATCH(1,A1:A3,0)",
            vec![5., 25., 38.],
            Value::Error(FormulaError::Na),
        ),
        (
            "=MATCH(25,A1:A3,-1)",
            vec![5., 25., 38.],
            Value::Error(FormulaError::Na),
        ),
        (
            "=MATCH(25,INDEX(A1:B3,0,1),0)",
            vec![5., 25., 38.],
            Value::Number(2.),
        ),
        (
            "=INDEX(A1:A3,MATCH(25,A1:A3,0))",
            vec![5., 25., 38.],
            Value::Number(25.),
        ),
    ] {
        let vector = Vector(values.into_iter().map(Value::Number).collect());
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &vector,
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    Ok(())
}

#[test]
fn match_types_blanks_errors_and_wildcards() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "=MATCH(0,A1:A3,0)",
            vec![Value::Blank, Value::Text("0".into()), Value::Number(-0.0)],
            Value::Number(3.),
        ),
        (
            "=MATCH(\"0\",A1:A3,0)",
            vec![Value::Blank, Value::Text("0".into()), Value::Number(0.)],
            Value::Number(2.),
        ),
        (
            "=MATCH(TRUE,A1:A3,0)",
            vec![
                Value::Number(1.),
                Value::Text("TRUE".into()),
                Value::Boolean(true),
            ],
            Value::Number(3.),
        ),
        (
            "=MATCH(\"a*?\",A1:A3,0)",
            vec![
                Value::Number(1.),
                Value::Text("a".into()),
                Value::Text("AB".into()),
            ],
            Value::Number(3.),
        ),
        (
            "=MATCH(\"~*\",A1:A3,0)",
            vec![
                Value::Text("ab".into()),
                Value::Text("*".into()),
                Value::Blank,
            ],
            Value::Number(2.),
        ),
        (
            "=MATCH(\"a?\",A1:A3,0)",
            vec![
                Value::Text("a😀".into()),
                Value::Text("ab".into()),
                Value::Blank,
            ],
            Value::Number(1.),
        ),
        (
            "=MATCH(\"\",A1:A3,0)",
            vec![Value::Blank, Value::Text("".into()), Value::Number(0.)],
            Value::Number(2.),
        ),
        (
            "=MATCH(7,A1:A3,0)",
            vec![
                Value::Number(7.),
                Value::Error(FormulaError::Div0),
                Value::Blank,
            ],
            Value::Number(1.),
        ),
        (
            "=MATCH(7,A1:A3,0)",
            vec![
                Value::Error(FormulaError::Div0),
                Value::Number(7.),
                Value::Blank,
            ],
            Value::Error(FormulaError::Div0),
        ),
        (
            "=MATCH(7,A1:A3,1)",
            vec![
                Value::Number(7.),
                Value::Error(FormulaError::Div0),
                Value::Number(9.),
            ],
            Value::Error(FormulaError::Div0),
        ),
        (
            "=MATCH(7,A3:A1,0)",
            vec![Value::Blank, Value::Number(7.), Value::Number(9.)],
            Value::Number(2.),
        ),
        (
            "=MATCH(0,A1:A3,0)",
            vec![Value::Blank, Value::Blank, Value::Blank],
            Value::Error(FormulaError::Na),
        ),
        (
            "=MATCH(7,A1:B3,0)",
            vec![Value::Number(7.)],
            Value::Error(FormulaError::Value),
        ),
        (
            "=MATCH(7,7,0)",
            vec![Value::Number(7.)],
            Value::Error(FormulaError::Value),
        ),
        (
            "=MATCH(7,A1:A3,2)",
            vec![Value::Number(7.)],
            Value::Error(FormulaError::Value),
        ),
        (
            "=IFERROR(MATCH(10,A1:A3,0),99)",
            vec![Value::Number(7.)],
            Value::Number(99.),
        ),
        (
            "=MATCH(\"b\",A1:A3,1)",
            vec![
                Value::Number(1.),
                Value::Text("a".into()),
                Value::Boolean(true),
            ],
            Value::Number(2.),
        ),
        (
            "=MATCH(\"b\",A1:A3,-1)",
            vec![
                Value::Boolean(true),
                Value::Text("a".into()),
                Value::Number(1.),
            ],
            Value::Number(1.),
        ),
        (
            "=MATCH(\"a\",A1:A3,1)",
            vec![Value::Number(1.), Value::Blank, Value::Text("a".into())],
            Value::Number(3.),
        ),
        (
            "=MATCH(\"?\",A1:A3,1)",
            vec![
                Value::Text("*".into()),
                Value::Text("?".into()),
                Value::Text("a".into()),
            ],
            Value::Number(2.),
        ),
        (
            "=MATCH(\"?\",A1:A3,0)",
            vec![
                Value::Text("İ".into()),
                Value::Text("a".into()),
                Value::Blank,
            ],
            Value::Number(2.),
        ),
        (
            "=MATCH(7,A1:A3,1)",
            vec![
                Value::Number(9.),
                Value::Number(7.),
                Value::Error(FormulaError::Div0),
            ],
            Value::Error(FormulaError::Div0),
        ),
        (
            "=MATCH(7,A1,0)",
            vec![Value::DateTime {
                serial: 7.,
                system: DateSystem::Excel1900,
            }],
            Value::Number(1.),
        ),
    ];
    for (source, values, expected) in cases {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &Vector(values),
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    Ok(())
}

#[test]
fn match_never_hides_boundary_failures_after_a_hit() -> Result<(), Box<dyn std::error::Error>> {
    for extra in [
        Value::Text("x".repeat(40000)),
        Value::Array {
            rows: 1,
            columns: 1,
            values: vec![Value::Number(1.)],
        },
    ] {
        let expected = if matches!(extra, Value::Array { .. }) {
            EvaluationError::UnsupportedCompatibilityFeature
        } else {
            EvaluationError::CalculationLimitExceeded
        };
        let ast = parse("=IFERROR(MATCH(7,A1:A2,0),99)")?.ast;
        assert_eq!(
            evaluate(
                &ast,
                &Vector(vec![Value::Number(7.), extra]),
                &mut EvaluationBudget::default()
            ),
            Err(expected)
        );
    }
    let ast = parse("=IFERROR(MATCH(7,A1:A100000,0),99)")?.ast;
    assert_eq!(
        evaluate(
            &ast,
            &Vector(vec![Value::Number(7.)]),
            &mut EvaluationBudget::new(100)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let mut budget = EvaluationBudget::default();
    budget.cancel();
    assert_eq!(
        evaluate(&ast, &Vector(vec![]), &mut budget),
        Err(EvaluationError::Cancelled)
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
    let ast = parse("=IFERROR(MATCH(7,A1:A2,0),99)")?.ast;
    for visits in [1, 3] {
        assert_eq!(
            evaluate(&ast, &Broken(visits), &mut EvaluationBudget::default()),
            Err(EvaluationError::InvalidResolverResult)
        );
    }
    let ast = parse("=IFERROR(MATCH(7,A1,A2),99)")?.ast;
    assert_eq!(
        evaluate(
            &ast,
            &Vector(vec![Value::Number(7.), Value::Text(" ".repeat(40000))]),
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let source = format!("=IFERROR(MATCH(\"{}\",A1,0),99)", "*a".repeat(16));
    assert_eq!(
        evaluate(
            &parse(&source)?.ast,
            &Vector(vec![Value::Text("a".repeat(200))]),
            &mut EvaluationBudget::new(1000)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}

proptest! {
    #[test]
    fn sorted_numeric_modes_match_position_oracle(mut values in prop::collection::vec(-100i16..100, 1..30), lookup in -110i16..110) {
        values.sort();
        for mode in [-1,0,1] {
            let mut ordered=values.clone();
            if mode == -1 {ordered.reverse();}
            let expected=if mode==0 {ordered.iter().position(|n|*n==lookup)} else {ordered.iter().rposition(|n|if mode==1 {*n<=lookup} else {*n>=lookup})};
            let source=format!("=MATCH({lookup},A1:A{},{mode})",ordered.len());
            let parsed=parse(&source).map_err(|e|TestCaseError::fail(e.to_string()))?;
            let vector=Vector(ordered.into_iter().map(|n|Value::Number(f64::from(n))).collect());
            let actual=evaluate(&parsed.ast,&vector,&mut EvaluationBudget::default()).map_err(|e|TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(actual,expected.map_or(Value::Error(FormulaError::Na),|n|Value::Number((n+1) as f64)));
        }
    }
}
