use spreadsheet_formula::*;
use std::cell::Cell;

struct Table {
    resolved: Cell<usize>,
    visited: Cell<usize>,
}
impl ReferenceResolver for Table {
    fn resolve_cell(&self, r: &CellReference) -> Value {
        self.resolved.set(self.resolved.get() + 1);
        match (r.row, r.column) {
            (0, 0) => Value::Number(7.0),
            (0, 1) => Value::Text("12".into()),
            (1, 0) => Value::Blank,
            (1, 1) => Value::Boolean(true),
            (2, 0) => Value::Error(FormulaError::Div0),
            (2, 1) => Value::Number(13.0),
            _ => Value::Blank,
        }
    }
    fn visit_range(
        &self,
        r: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        for row in r.start.row.min(r.end.row)..=r.start.row.max(r.end.row) {
            for column in r.start.column.min(r.end.column)..=r.start.column.max(r.end.column) {
                self.visited.set(self.visited.get() + 1);
                visitor(self.resolve_cell(&CellReference {
                    row,
                    column,
                    ..r.start.clone()
                }))?;
            }
        }
        Ok(())
    }
}
fn table() -> Table {
    Table {
        resolved: Cell::new(0),
        visited: Cell::new(0),
    }
}

#[test]
fn index_selects_only_target_and_preserves_reference_coercion()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        ("=INDEX(A1:B3,3,2)", Value::Number(13.0)),
        ("=INDEX(B3:A1,1,1)", Value::Number(7.0)),
        ("=INDEX(A1:A3,2)", Value::Number(0.0)),
        ("=INDEX(A1:B3,1,2)", Value::Text("12".into())),
        ("=INDEX(A1:B3,2,2)", Value::Boolean(true)),
        ("=SUM(INDEX(A1:B3,1,2))", Value::Number(0.0)),
        ("=COUNT(INDEX(A1:A3,2))", Value::Number(0.0)),
        ("=COUNTA(INDEX(A1:A3,2))", Value::Number(0.0)),
        ("=COUNT(INDEX(A1:B3,2,2))", Value::Number(0.0)),
        ("=COUNTA(INDEX(A1:B3,2,2))", Value::Number(1.0)),
        ("=SUM(INDEX(A1:B3,0,2))", Value::Number(13.0)),
        ("=AVERAGE(INDEX(A1:B3,0,2))", Value::Number(13.0)),
        ("=MIN(INDEX(A1:B3,0,2))", Value::Number(13.0)),
        ("=MAX(INDEX(A1:B3,0,2))", Value::Number(13.0)),
        ("=COUNT(INDEX(A1:B3,0,2))", Value::Number(1.0)),
        ("=COUNTA(INDEX(A1:B3,0,2))", Value::Number(3.0)),
        ("=AND(INDEX(A1:B3,0,2))", Value::Boolean(true)),
        ("=OR(INDEX(A1:B3,2,0))", Value::Boolean(true)),
        ("=INDEX(INDEX(A1:B3,0,2),3)", Value::Number(13.0)),
    ];
    for (source, expected) in cases {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &table(),
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    let resolver = table();
    assert_eq!(
        evaluate(
            &parse("=INDEX(A1:A100000,1)")?.ast,
            &resolver,
            &mut EvaluationBudget::new(20)
        )?,
        Value::Number(7.0)
    );
    assert_eq!(resolver.resolved.get(), 1);
    assert_eq!(resolver.visited.get(), 0);
    Ok(())
}

#[test]
fn index_bounds_missing_selectors_and_typed_limits() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected) in [
        ("=INDEX(A1:B3,4,1)", FormulaError::Ref),
        ("=INDEX(A1:B3,1,3)", FormulaError::Ref),
        ("=INDEX(A1:B3,-0.1,1)", FormulaError::Value),
        ("=INDEX(A1:B3,1,1,0)", FormulaError::Ref),
        ("=INDEX(A1:B3,1,1,2)", FormulaError::Ref),
        ("=INDEX(A1:B3,1E200,1)", FormulaError::Ref),
        ("=INDEX(A1:B3,\"bad\",1)", FormulaError::Value),
        ("=INDEX(A1:B3)", FormulaError::Value),
        ("=INDEX(A1:B3,1,1,1,1)", FormulaError::Value),
        ("=INDEX(A1:B3,3,1)", FormulaError::Div0),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &table(),
                &mut EvaluationBudget::default()
            )?,
            Value::Error(expected),
            "{source}"
        );
    }
    for (source, expected) in [
        ("=INDEX(A1:B3,\"3\",\"2\")", Value::Number(13.0)),
        ("=INDEX(A1:B3,2.9,1)", Value::Number(0.0)),
        ("=INDEX(A1:B3,TRUE,TRUE)", Value::Number(7.0)),
        ("=SUM(INDEX(A1:B3,,2))", Value::Number(13.0)),
        ("=SUM(INDEX(A1:B3,2,))", Value::Number(0.0)),
        ("=COUNTA(INDEX(A1:B3,0,0,))", Value::Number(5.0)),
        ("=COUNT(INDEX(A1:B3,3,1))", Value::Number(0.0)),
        ("=COUNTA(INDEX(A1:B3,3,1))", Value::Number(1.0)),
        ("=IFERROR(INDEX(A1:B3,4,1),9)", Value::Number(9.0)),
        ("=INDEX(42,1)", Value::Number(42.0)),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &table(),
                &mut EvaluationBudget::default()
            )?,
            expected,
            "{source}"
        );
    }
    for source in [
        "=INDEX(A1:B3,0,2)",
        "=INDEX(A1:B3,2)",
        "=INDEX(A1:B1,2)",
        "=IFERROR(INDEX(A1:B3,,2,1),9)",
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &table(),
                &mut EvaluationBudget::default()
            ),
            Err(EvaluationError::UnsupportedCompatibilityFeature),
            "{source}"
        );
    }
    let mut ast = parse("=IFERROR(INDEX(A1:A100000,1),9)")?.ast;
    for node in &mut ast.nodes {
        if let Node::Range(range) = node {
            range.end.row = 100000;
        }
    }
    assert_eq!(
        evaluate(&ast, &table(), &mut EvaluationBudget::default()),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let ast = parse("=IFERROR(SUM(INDEX(A1:B3,0,2)),9)")?.ast;
    assert_eq!(
        evaluate(&ast, &table(), &mut EvaluationBudget::new(3)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let mut cancelled = EvaluationBudget::default();
    cancelled.cancel();
    assert_eq!(
        evaluate(&ast, &table(), &mut cancelled),
        Err(EvaluationError::Cancelled)
    );
    Ok(())
}

#[test]
fn index_rejects_incomplete_resolver_and_charges_numeric_text()
-> Result<(), Box<dyn std::error::Error>> {
    struct Bad {
        visits: usize,
        bytes: usize,
    }
    impl ReferenceResolver for Bad {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Text(format!("{}1", " ".repeat(self.bytes)))
        }
        fn visit_range(
            &self,
            _: &RangeReference,
            visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            for _ in 0..self.visits {
                visitor(Value::Number(1.0))?;
            }
            Ok(())
        }
    }
    let ast = parse("=IFERROR(SUM(INDEX(A1:A3,0)),9)")?.ast;
    for visits in [0, 1, 4] {
        assert_eq!(
            evaluate(
                &ast,
                &Bad { visits, bytes: 0 },
                &mut EvaluationBudget::default()
            ),
            Err(EvaluationError::InvalidResolverResult)
        );
    }
    let ast = parse("=IFERROR(INDEX(B1:B3,A1),9)")?.ast;
    assert_eq!(
        evaluate(
            &ast,
            &Bad {
                visits: 0,
                bytes: 40000
            },
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    assert_eq!(
        evaluate(
            &ast,
            &Bad {
                visits: 0,
                bytes: 100
            },
            &mut EvaluationBudget::new(50)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}

#[test]
fn selected_arrays_stay_typed_unsupported_even_inside_iferror()
-> Result<(), Box<dyn std::error::Error>> {
    struct Arrays;
    impl ReferenceResolver for Arrays {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Array {
                rows: 1,
                columns: 1,
                values: vec![Value::Number(1.0)],
            }
        }
        fn visit_range(
            &self,
            range: &RangeReference,
            visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            visitor(self.resolve_cell(&range.start))
        }
    }
    for source in [
        "=INDEX(A1,1)",
        "=SUM(INDEX(A1,1))",
        "=AVERAGE(INDEX(A1,1))",
        "=MIN(INDEX(A1,1))",
        "=MAX(INDEX(A1,1))",
        "=COUNT(INDEX(A1,1))",
        "=AND(INDEX(A1,1))",
        "=OR(INDEX(A1,1))",
    ] {
        let wrapped = format!("=IFERROR({},9)", &source[1..]);
        assert_eq!(
            evaluate(
                &parse(&wrapped)?.ast,
                &Arrays,
                &mut EvaluationBudget::default()
            ),
            Err(EvaluationError::UnsupportedCompatibilityFeature),
            "{source}"
        );
    }
    Ok(())
}
