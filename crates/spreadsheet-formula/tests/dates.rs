use proptest::prelude::*;
use spreadsheet_formula::*;
struct CalendarResolver {
    system: DateSystem,
    cell: Value,
}
impl ReferenceResolver for CalendarResolver {
    fn date_system(&self) -> DateSystem {
        self.system
    }
    fn resolve_cell(&self, _: &CellReference) -> Value {
        self.cell.clone()
    }
    fn visit_range(
        &self,
        _: &RangeReference,
        _: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        Err(EvaluationError::UnsupportedCompatibilityFeature)
    }
}
#[test]
fn explicit_1904_and_datetime_system() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = CalendarResolver {
        system: DateSystem::Excel1904,
        cell: Value::DateTime {
            serial: 60.5,
            system: DateSystem::Excel1900,
        },
    };
    for (source, expected) in [
        ("=DATE(1904,1,1)", 0.0),
        ("=DATE(2000,1,1)", 35064.0),
        ("=DATE(9999,12,31)", 2957003.0),
        ("=YEAR(0)", 1904.0),
        ("=DAY(0)", 1.0),
        ("=MONTH(60)", 3.0),
        ("=DAY(60)", 1.0),
        ("=DAY(A1)", 29.0),
    ] {
        assert_eq!(
            evaluate(
                &parse(source)?.ast,
                &resolver,
                &mut EvaluationBudget::default()
            )?,
            Value::Number(expected)
        );
    }
    Ok(())
}
#[test]
fn coercion_limits_and_errors() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected) in [
        ("=DATE(\"2000\",\" 2 \",\"29\")", 36585.0),
        ("=DATE(2000.9,2.9,29.9)", 36585.0),
        ("=DATE(2000,0,1)", 36495.0),
        ("=DATE(2000,1,0)", 36525.0),
        ("=DATE(1900,1,0)", 0.0),
    ] {
        assert_eq!(eval(source)?, Value::Number(expected), "{source}");
    }
    for source in [
        "=DATE()",
        "=DATE(2000,1)",
        "=YEAR(1,2)",
        "=DAY(\"2000-01-01\")",
    ] {
        assert_eq!(eval(source)?, Value::Error(FormulaError::Value));
    }
    assert_eq!(eval("=DATE(1/0,1,1)")?, Value::Error(FormulaError::Div0));
    for cell in [
        Value::Text(" ".repeat(MAX_SOURCE_BYTES + 1)),
        Value::Text(" ".repeat(100)),
    ] {
        let resolver = CalendarResolver {
            system: DateSystem::Excel1900,
            cell,
        };
        assert_eq!(
            evaluate(
                &parse("=IFERROR(YEAR(A1),0)")?.ast,
                &resolver,
                &mut EvaluationBudget::new(20)
            ),
            Err(EvaluationError::CalculationLimitExceeded)
        );
    }
    let mut budget = EvaluationBudget::default();
    budget.cancel();
    assert_eq!(
        evaluate(&parse("=DATE(2000,1,1)")?.ast, &NoReferences, &mut budget),
        Err(EvaluationError::Cancelled)
    );
    Ok(())
}
#[test]
fn fractional_and_integer_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    // These fractional conventions are project-profile cases, not Excel goldens.
    for (source, expected) in [
        ("=DATE(-0.9,1,1)", 1.0),
        ("=DATE(2000,0.9,1)", 36495.0),
        ("=DATE(2000,1,-0.9)", 36525.0),
        ("=DATE(1900,2,30)", 61.0),
        ("=DATE(1900,3,-1)", 59.0),
        ("=DATE(1900,4,-30)", 61.0),
    ] {
        assert_eq!(eval(source)?, Value::Number(expected), "{source}");
    }
    for source in [
        "=DATE(2000,9223372036854775808,1)",
        "=DATE(2000,-9223372036854775808,1)",
        "=DATE(2000,1,9223372036854775808)",
        "=DATE(2000,1,-9223372036854775808)",
    ] {
        assert_eq!(eval(source)?, Value::Error(FormulaError::Num), "{source}");
    }
    Ok(())
}
proptest! {
    #[test]
    fn serial_roundtrip(value in 0i64..2957004, use_1904 in any::<bool>()) {
        let resolver=CalendarResolver{system:if use_1904 {DateSystem::Excel1904} else {DateSystem::Excel1900},cell:Value::Blank};
        let source=format!("=DATE(YEAR({value}),MONTH({value}),DAY({value}))");
        let parsed=parse(&source);prop_assert!(parsed.is_ok());
        if let Ok(parsed)=parsed { prop_assert_eq!(evaluate(&parsed.ast,&resolver,&mut EvaluationBudget::default()),Ok(Value::Number(value as f64))); }
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
fn excel_1900_calendar_profile() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected) in [
        ("=DATE(1900,1,1)", 1.0),
        ("=DATE(1900,2,28)", 59.0),
        ("=DATE(1900,2,29)", 60.0),
        ("=DATE(1900,3,1)", 61.0),
        ("=DATE(1900,3,0)", 60.0),
        ("=DATE(1900,1,61)", 61.0),
        ("=YEAR(60)", 1900.0),
        ("=MONTH(60)", 2.0),
        ("=DAY(60)", 29.0),
        ("=YEAR(0)", 1900.0),
        ("=MONTH(0)", 1.0),
        ("=DAY(0)", 0.0),
        ("=DATE(1904,1,1)", 1462.0),
        ("=DATE(9999,12,31)", 2958465.0),
        ("=DATE(9999,13,0)", 2958465.0),
        ("=YEAR(2958465.9)", 9999.0),
        ("=DATE(0,1,1)", 1.0),
        ("=YEAR(DATE(2000,2,29))", 2000.0),
        ("=MONTH(DATE(2100,2,29))", 3.0),
    ] {
        assert_eq!(eval(source)?, Value::Number(expected), "{source}");
    }
    for source in [
        "=DATE(10000,1,1)",
        "=DATE(-1,1,1)",
        "=DATE(2000,1e308,1)",
        "=YEAR(-1)",
        "=DAY(2958466)",
    ] {
        assert_eq!(eval(source)?, Value::Error(FormulaError::Num));
    }
    Ok(())
}
