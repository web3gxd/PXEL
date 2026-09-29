use proptest::prelude::*;
use spreadsheet_formula::*;

struct FourCells {
    visits: usize,
}

#[test]
fn numeric_text_indices_and_case_expansion_cannot_hide_limits()
-> Result<(), Box<dyn std::error::Error>> {
    struct Index {
        bytes: usize,
    }
    impl ReferenceResolver for Index {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Text(format!("{}1", " ".repeat(self.bytes)))
        }
        fn visit_range(
            &self,
            _: &RangeReference,
            _: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            Err(EvaluationError::UnsupportedCompatibilityFeature)
        }
    }
    let ast = parse("=IFERROR(LEFT(\"x\",A1),\"fallback\")")?.ast;
    assert_eq!(
        evaluate(
            &ast,
            &Index { bytes: 40000 },
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    assert_eq!(
        evaluate(&ast, &Index { bytes: 100 }, &mut EvaluationBudget::new(50)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    struct Expansion;
    impl ReferenceResolver for Expansion {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Text("İ".repeat(15000))
        }
        fn visit_range(
            &self,
            _: &RangeReference,
            _: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            Err(EvaluationError::UnsupportedCompatibilityFeature)
        }
    }
    assert_eq!(
        evaluate(
            &parse("=IFERROR(LOWER(A1),\"fallback\")")?.ast,
            &Expansion,
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}
impl ReferenceResolver for FourCells {
    fn resolve_cell(&self, _: &CellReference) -> Value {
        Value::Blank
    }
    fn visit_range(
        &self,
        _: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        for value in [
            Value::Text("a".into()),
            Value::Blank,
            Value::Number(12.0),
            Value::Boolean(true),
        ]
        .into_iter()
        .take(self.visits)
        {
            visitor(value)?;
        }
        Ok(())
    }
}
#[test]
fn concat_ranges_and_resource_errors_are_checked() -> Result<(), Box<dyn std::error::Error>> {
    let ast = parse("=CONCAT(A1:B2)")?.ast;
    assert_eq!(
        evaluate(
            &ast,
            &FourCells { visits: 4 },
            &mut EvaluationBudget::default()
        )?,
        Value::Text("a12TRUE".into())
    );
    assert_eq!(
        evaluate(
            &ast,
            &FourCells { visits: 3 },
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::InvalidResolverResult)
    );
    let source = format!(
        "=IFERROR(CONCAT(\"{}\",\"{}\"),\"fallback\")",
        "a".repeat(16000),
        "a".repeat(16000)
    );
    assert_eq!(eval(&source)?, Value::Text("a".repeat(32000)));
    let source = format!(
        "=IFERROR(CONCAT(\"{}\",\"{}\"),\"fallback\")",
        "a".repeat(16400),
        "a".repeat(16400)
    );
    // Source itself exceeds parser cap: test expansion through references instead.
    assert!(parse(&source).is_err());
    struct Large;
    impl ReferenceResolver for Large {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Text("a".repeat(17000))
        }
        fn visit_range(
            &self,
            _: &RangeReference,
            _: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            Err(EvaluationError::UnsupportedCompatibilityFeature)
        }
    }
    assert_eq!(
        evaluate(
            &parse("=CONCAT(A1,A1)")?.ast,
            &Large,
            &mut EvaluationBudget::default()
        )?,
        Value::Error(FormulaError::Value)
    );
    struct UnicodeLarge;
    impl ReferenceResolver for UnicodeLarge {
        fn resolve_cell(&self, _: &CellReference) -> Value {
            Value::Text("😀".repeat(5000))
        }
        fn visit_range(
            &self,
            _: &RangeReference,
            _: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
        ) -> Result<(), EvaluationError> {
            Err(EvaluationError::UnsupportedCompatibilityFeature)
        }
    }
    assert_eq!(
        evaluate(
            &parse("=IFERROR(CONCAT(A1,A1),\"fallback\")")?.ast,
            &UnicodeLarge,
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}

proptest! {
    #[test]
    fn left_right_preserve_unicode_split(input in ".{0,80}", split in 0usize..100) {
        let count=input.chars().count(); let split=split.min(count);
        let escaped=input.replace('"',"\"\"");
        let formula=format!("=CONCAT(LEFT(\"{escaped}\",{split}),RIGHT(\"{escaped}\",{}))",count-split);
        prop_assert!(matches!(eval(&formula),Ok(Value::Text(result)) if result==input));
    }
}
fn eval(source: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(evaluate(
        &parse(source)?.ast,
        &NoReferences,
        &mut EvaluationBudget::default(),
    )?)
}
#[test]
fn unicode_text_contract() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected) in [
        ("=CONCAT(\"a\",TRUE,12,)", "aTRUE12"),
        ("=LEFT(\"😀аб\",2)", "😀а"),
        ("=RIGHT(\"😀аб\",2)", "аб"),
        ("=MID(\"😀аб\",2,1)", "а"),
        ("=LEFT(\"abc\")", "a"),
        ("=RIGHT(\"abc\")", "c"),
        ("=MID(\"abc\",1e308,1e308)", ""),
        ("=LEFT(\"abc\",1e308)", "abc"),
        ("=TRIM(\"  a  b  \" )", "a b"),
        ("=TRIM(\"  a\t  b  \" )", "  a\t b "),
        ("=UPPER(\"straße\")", "STRASSE"),
        ("=LOWER(\"ПРИВЕТ\")", "привет"),
        ("=LEFT(\"abc\",1.9)", "a"),
    ] {
        assert_eq!(eval(source)?, Value::Text(expected.into()));
    }
    assert_eq!(eval("=LEN(\"😀é️\")")?, Value::Number(4.0));
    for source in [
        "=LEFT(\"abc\",-0.1)",
        "=MID(\"abc\",0.9,1)",
        "=LEN()",
        "=CONCAT()",
    ] {
        assert_eq!(eval(source)?, Value::Error(FormulaError::Value));
    }
    assert_eq!(
        eval("=IFERROR(LEFT(\"x\",-1),\"ok\")")?,
        Value::Text("ok".into())
    );
    assert_eq!(
        evaluate(
            &parse("=IFERROR(UPPER(\"abcdef\"),\"ok\")")?.ast,
            &NoReferences,
            &mut EvaluationBudget::new(3)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}
