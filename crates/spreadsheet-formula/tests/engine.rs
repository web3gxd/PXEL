use proptest::prelude::*;
use spreadsheet_formula::*;

fn eval(text: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(evaluate(
        &parse(text)?.ast,
        &NoReferences,
        &mut EvaluationBudget::default(),
    )?)
}

#[test]
fn abs_and_mod_scalar_contract() -> Result<(), Box<dyn std::error::Error>> {
    for (formula, expected) in [
        ("=ABS(-3)", Value::Number(3.0)),
        ("=ABS(-0)", Value::Number(0.0)),
        ("=ABS(\"-2.5\")", Value::Number(2.5)),
        ("=ABS(TRUE)", Value::Number(1.0)),
        ("=ABS()", Value::Error(FormulaError::Value)),
        ("=ABS(1,2)", Value::Error(FormulaError::Value)),
        ("=MOD(3,2)", Value::Number(1.0)),
        ("=MOD(-3,2)", Value::Number(1.0)),
        ("=MOD(3,-2)", Value::Number(-1.0)),
        ("=MOD(-3,-2)", Value::Number(-1.0)),
        ("=MOD(5.5,2)", Value::Number(1.5)),
        ("=MOD(-4,2)", Value::Number(0.0)),
        ("=MOD(3,0)", Value::Error(FormulaError::Div0)),
        ("=MOD(1e308,1e-308)", Value::Error(FormulaError::Num)),
        ("=MOD(\"x\",2)", Value::Error(FormulaError::Value)),
        ("=MOD(3)", Value::Error(FormulaError::Value)),
        ("=IFERROR(MOD(1,0),7)", Value::Number(7.0)),
    ] {
        assert_eq!(eval(formula)?, expected, "{formula}");
    }
    let zero = eval("=ABS(-0)")?;
    assert!(matches!(zero, Value::Number(value) if value.to_bits() == 0));
    let remainder_zero = eval("=MOD(-4,2)")?;
    assert!(matches!(remainder_zero, Value::Number(value) if value.to_bits() == 0));
    assert_eq!(
        evaluate(
            &parse("=IFERROR(MOD(1,2),7)")?.ast,
            &NoReferences,
            &mut EvaluationBudget::new(3)
        ),
        Err(EvaluationError::CalculationLimitExceeded),
    );
    Ok(())
}

#[test]
fn power_supports_fractional_exponents_and_explicit_domain_errors()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        evaluate(
            &parse("=IFERROR(POWER(4,0.5),7)")?.ast,
            &NoReferences,
            &mut EvaluationBudget::new(40)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    for (formula, expected) in [
        ("=POWER(5,2)", Value::Number(25.0)),
        ("=POWER(4,0.5)", Value::Number(2.0)),
        ("=4^0.5", Value::Number(2.0)),
        ("=POWER(-2,3)", Value::Number(-8.0)),
        ("=POWER(-2,-3)", Value::Number(-0.125)),
        ("=POWER(0,0)", Value::Error(FormulaError::Num)),
        ("=POWER(0,-1)", Value::Error(FormulaError::Div0)),
        ("=POWER(-2,0.5)", Value::Error(FormulaError::Num)),
        ("=POWER(2,1025)", Value::Error(FormulaError::Num)),
        ("=POWER(2,-1074)", Value::Number(f64::from_bits(1))),
        ("=POWER(2,-1075)", Value::Number(0.0)),
        ("=POWER(\"4\",TRUE)", Value::Number(4.0)),
        ("=POWER(1)", Value::Error(FormulaError::Value)),
        ("=POWER(\"x\",1)", Value::Error(FormulaError::Value)),
        ("=IFERROR(POWER(-4,0.5),7)", Value::Number(7.0)),
    ] {
        assert_eq!(eval(formula)?, expected, "{formula}");
    }
    assert!(
        matches!(eval("=POWER(4,5/4)")?, Value::Number(value) if (value-5.656854249492381).abs()<1e-14)
    );
    Ok(())
}

#[test]
// Microsoft ROUNDUP/ROUNDDOWN examples deliberately use rounded decimal PI inputs.
#[allow(clippy::approx_constant)]
fn decimal_rounding_profile() -> Result<(), Box<dyn std::error::Error>> {
    for source in ["=ROUND(-0.1,0)", "=ROUNDDOWN(-5e-324,323)"] {
        assert!(matches!(eval(source)?, Value::Number(value) if value.to_bits() == 0));
    }
    for (source, expected) in [
        ("=ROUND(2.15,1)", Value::Number(2.2)),
        ("=ROUND(2.149,1)", Value::Number(2.1)),
        ("=ROUND(-1.475,2)", Value::Number(-1.48)),
        ("=ROUND(626.3,-3)", Value::Number(1000.0)),
        ("=ROUND(1.005,2)", Value::Number(1.01)),
        ("=ROUND(0.49999999999999994,0)", Value::Number(0.0)),
        ("=ROUNDUP(-3.14159,3)", Value::Number(-3.142)),
        ("=ROUNDDOWN(-3.14159,3)", Value::Number(-3.141)),
        ("=ROUND(123.45,-1.9)", Value::Number(120.0)),
        ("=ROUND(1e308,308)", Value::Number(1e308)),
        ("=ROUND(5e-324,323)", Value::Number(1e-323)),
        ("=ROUNDDOWN(5e-324,323)", Value::Number(0.0)),
        ("=ROUNDUP(1,-309)", Value::Error(FormulaError::Num)),
        ("=ROUND(1,-309)", Value::Number(0.0)),
        ("=ROUND(1e308,1e308)", Value::Number(1e308)),
        ("=ROUND(1)", Value::Error(FormulaError::Value)),
        ("=ROUND(\"x\",1)", Value::Error(FormulaError::Value)),
    ] {
        assert_eq!(eval(source)?, expected, "{source}");
    }
    assert_eq!(
        evaluate(
            &parse("=IFERROR(ROUND(1.005,2),7)")?.ast,
            &NoReferences,
            &mut EvaluationBudget::new(5)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    Ok(())
}

#[test]
fn excel_verified_number_precision_and_rounding_golden() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/formulas/excel-number-precision-2026-10-02.json"
    ))?;
    assert_eq!(fixture["verified_in_excel"], true);
    for case in fixture["cases"].as_array().ok_or("missing Excel cases")? {
        let formula = case["formula"].as_str().ok_or("missing formula")?;
        let expected = match case["result_type"].as_str().ok_or("missing result type")? {
            "number" => Value::Number(case["result"].as_f64().ok_or("missing numeric result")?),
            "error" if case["result"] == "#NUM!" => Value::Error(FormulaError::Num),
            _ => return Err("unsupported Excel fixture result".into()),
        };
        let actual = eval(formula)?;
        if let Some(tolerance) = case["tolerance"].as_f64() {
            match (&actual, &expected) {
                (Value::Number(actual), Value::Number(expected)) => assert!(
                    (actual - expected).abs() <= tolerance,
                    "Excel COM observation: {formula}; actual={actual:.17e}, expected={expected:.17e}, tolerance={tolerance:.1e}"
                ),
                _ => assert_eq!(actual, expected, "Excel COM observation: {formula}"),
            }
        } else {
            assert_eq!(actual, expected, "Excel COM observation: {formula}");
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn rounding_thousandths_matches_integer_quantization(n in -1_000_000i64..1_000_000, digits in 0u32..=3) {
        let divisor = 10i64.pow(3-digits);
        let magnitude = n.abs();
        let sign = if n < 0 { -1.0 } else { 1.0 };
        let value = n as f64 / 1000.0;
        for (name, units) in [
            ("ROUND", (magnitude + divisor/2) / divisor),
            ("ROUNDUP", (magnitude + divisor-1) / divisor),
            ("ROUNDDOWN", magnitude / divisor),
        ] {
            let expected = sign * (units * divisor) as f64 / 1000.0;
            let actual = eval(&format!("={name}({value},{digits})"));
            prop_assert!(matches!(actual, Ok(Value::Number(result)) if result == expected));
        }
    }
}

proptest! {
    #[test]
    fn mod_integer_domain_matches_exact_divisor_signed_remainder(n in -1_000_000_000i64..1_000_000_000, magnitude in 1i64..1_000_000, negative in any::<bool>()) {
        let divisor = if negative { -magnitude } else { magnitude };
        let mut expected = n % divisor;
        if expected != 0 && (expected < 0) != (divisor < 0) { expected += divisor; }
        let actual = eval(&format!("=MOD({n},{divisor})"));
        prop_assert!(matches!(actual, Ok(Value::Number(value)) if value == expected as f64));
    }
}

#[test]
fn scalar_contract_and_lazy_if() -> Result<(), Box<dyn std::error::Error>> {
    for (formula, expected) in [
        ("=1+2*3", Value::Number(7.0)),
        ("=-2^2", Value::Number(4.0)),
        ("=2^3^2", Value::Number(64.0)),
        ("=IF(FALSE,1/0,7)", Value::Number(7.0)),
        ("=IF(TRUE,,9)", Value::Number(0.0)),
        ("=IF(FALSE,9)", Value::Boolean(false)),
        ("=IF(FALSE,9,)", Value::Number(0.0)),
        ("=SUM(TRUE,\"5\",2)", Value::Number(8.0)),
        ("=SUM(1,,2)", Value::Number(3.0)),
        ("=1/0", Value::Error(FormulaError::Div0)),
        ("=FOO(1)", Value::Error(FormulaError::Name)),
        ("=1<2", Value::Boolean(true)),
        ("=2^-1024", Value::Number(f64::from_bits(1 << 50))),
    ] {
        assert_eq!(eval(formula)?, expected, "{formula}");
    }
    Ok(())
}

#[test]
fn ifs_selects_first_true_pair_without_evaluating_later_pairs()
-> Result<(), Box<dyn std::error::Error>> {
    for (formula, expected) in [
        ("=IFS(FALSE,1/0,TRUE,9)", Value::Number(9.0)),
        ("=IFS(TRUE,1,1/0,2)", Value::Number(1.0)),
        ("=IFS(FALSE,1,FALSE,2)", Value::Error(FormulaError::Na)),
        ("=IFS(1,2)", Value::Error(FormulaError::Value)),
        ("=IFS(TRUE,)", Value::Number(0.0)),
    ] {
        assert_eq!(eval(formula)?, expected, "{formula}");
    }
    let max_pairs = format!("=IFS({}TRUE,7)", "0=1,0,".repeat(126));
    assert_eq!(eval(&max_pairs)?, Value::Number(7.0));
    let too_many = format!("=IFS({}TRUE,7)", "0=1,0,".repeat(127));
    assert!(parse(&too_many).is_err());
    Ok(())
}

#[test]
fn logical_functions_evaluate_all_conditions_and_validate_arity()
-> Result<(), Box<dyn std::error::Error>> {
    for (formula, expected) in [
        ("=AND(TRUE,2,-3)", Value::Boolean(true)),
        ("=AND(TRUE,0)", Value::Boolean(false)),
        ("=OR(FALSE,0,-3)", Value::Boolean(true)),
        ("=OR(FALSE,0)", Value::Boolean(false)),
        ("=AND(\"TRUE\",TRUE)", Value::Boolean(true)),
        ("=OR(\"FALSE\",FALSE)", Value::Boolean(false)),
        ("=AND(TRUE,\"word\")", Value::Error(FormulaError::Value)),
        ("=AND(FALSE,1/0)", Value::Error(FormulaError::Div0)),
        ("=OR(TRUE,1/0)", Value::Error(FormulaError::Div0)),
        ("=AND()", Value::Error(FormulaError::Value)),
        ("=OR()", Value::Error(FormulaError::Value)),
        ("=NOT(TRUE)", Value::Boolean(false)),
        ("=NOT(0)", Value::Boolean(true)),
        ("=NOT(-3)", Value::Boolean(false)),
        ("=NOT(1/0)", Value::Error(FormulaError::Div0)),
        ("=NOT()", Value::Error(FormulaError::Value)),
        ("=NOT(TRUE,FALSE)", Value::Error(FormulaError::Value)),
    ] {
        assert_eq!(eval(formula)?, expected, "{formula}");
    }
    for function in ["AND", "OR"] {
        assert_eq!(
            eval(&format!("={function}({}TRUE)", "TRUE,".repeat(254)))?,
            Value::Boolean(true)
        );
        let ast = parse(&format!("=IFERROR({function}(A1:A100000),7)"))?.ast;
        assert_eq!(
            evaluate(&ast, &Ones, &mut EvaluationBudget::new(100)),
            Err(EvaluationError::CalculationLimitExceeded)
        );
    }
    Ok(())
}

#[test]
fn iferror_does_not_turn_work_exhaustion_into_a_fallback() -> Result<(), Box<dyn std::error::Error>>
{
    let ast = parse("=IFERROR(SUM(A1:A100000),7)")?.ast;
    assert_eq!(
        evaluate(&ast, &Ones, &mut EvaluationBudget::new(100)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let mut cancelled = EvaluationBudget::default();
    cancelled.cancel();
    assert_eq!(
        evaluate(&ast, &Ones, &mut cancelled),
        Err(EvaluationError::Cancelled)
    );
    Ok(())
}

struct Ones;
impl ReferenceResolver for Ones {
    fn resolve_cell(&self, _: &CellReference) -> Value {
        Value::Number(1.0)
    }
    fn visit_range(
        &self,
        reference: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        for _ in 0..reference.cell_count() {
            visitor(Value::Number(1.0))?;
        }
        Ok(())
    }
}

struct FailedRange(EvaluationError);
impl ReferenceResolver for FailedRange {
    fn resolve_cell(&self, _: &CellReference) -> Value {
        Value::Error(FormulaError::Ref)
    }
    fn visit_range(
        &self,
        _: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        visitor(Value::Error(FormulaError::Ref))?;
        Err(self.0)
    }
}
#[test]
fn range_resource_failure_is_never_hidden_by_a_spreadsheet_error()
-> Result<(), Box<dyn std::error::Error>> {
    for function in ["SUM", "COUNT", "COUNTA", "AND", "OR"] {
        let ast = parse(&format!("={function}(A1:A2)"))?.ast;
        for error in [
            EvaluationError::Cancelled,
            EvaluationError::CalculationLimitExceeded,
            EvaluationError::InvalidResolverResult,
        ] {
            assert_eq!(
                evaluate(&ast, &FailedRange(error), &mut EvaluationBudget::default()),
                Err(error)
            );
        }
    }
    Ok(())
}
#[test]
fn range_work_is_charged_and_cancellation_is_not_a_spreadsheet_value()
-> Result<(), Box<dyn std::error::Error>> {
    let ast = parse("=SUM(A1:A100000)")?.ast;
    assert_eq!(
        evaluate(&ast, &Ones, &mut EvaluationBudget::default())?,
        Value::Number(100000.0)
    );
    assert_eq!(
        evaluate(&ast, &Ones, &mut EvaluationBudget::new(100)),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let mut cancelled = EvaluationBudget::default();
    cancelled.cancel();
    assert_eq!(
        evaluate(&ast, &Ones, &mut cancelled),
        Err(EvaluationError::Cancelled)
    );
    assert_eq!(
        evaluate(
            &parse("=2^0.5")?.ast,
            &NoReferences,
            &mut EvaluationBudget::default()
        ),
        Ok(Value::Number(std::f64::consts::SQRT_2))
    );
    assert!(parse("=SUM(A1:A100001)").is_err());
    Ok(())
}

#[test]
fn canonical_roundtrip_preserves_references_escaping_and_missing_args()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "='My '' sheet'!$A2+B$3",
        "=IF(TRUE,,)",
        "=sum(a1:$B$5)",
        "=\"a\"\"b\"&\"x\"",
        "=-(1+2)%",
    ] {
        let first = canonical(&parse(source)?.ast)?;
        let second = canonical(&parse(&first)?.ast)?;
        assert_eq!(first, second);
    }
    Ok(())
}

#[test]
fn canonical_parentheses_do_not_reject_admitted_unary_depth()
-> Result<(), Box<dyn std::error::Error>> {
    let source = format!("={}1", "-".repeat(100));
    let first = canonical(&parse(&source)?.ast)?;
    assert_eq!(canonical(&parse(&first)?.ast)?, first);
    Ok(())
}

proptest! {
    #[test]
    fn arbitrary_utf8_never_panics(input in ".{0,2048}") { let _ = parse(&input); let _ = parse(&format!("={input}")); }
    #[test]
    fn canonicalization_is_idempotent(a in -1000i32..1000, b in -1000i32..1000) {
        let first = canonical(&parse(&format!("=({a})+({b})*2"))?.ast)?;
        prop_assert_eq!(&first, &canonical(&parse(&first)?.ast)?);
    }
}

#[test]
fn malformed_public_ast_and_deep_input_fail_with_typed_limits()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        format!("={}1{}", "(".repeat(4096), ")".repeat(4096)),
        format!("={}1", "-".repeat(4096)),
        format!("={}", "1+".repeat(4096)),
    ] {
        assert!(parse(&source).is_err());
    }
    let start = CellReference {
        sheet: None,
        row: 0,
        column: 0,
        row_absolute: false,
        column_absolute: false,
    };
    let range = RangeReference {
        start: start.clone(),
        end: CellReference {
            row: u32::MAX,
            column: u32::MAX,
            ..start
        },
    };
    assert_eq!(range.cell_count(), u64::MAX);
    let ast = Ast {
        schema_version: 1,
        root: NodeId(1),
        nodes: vec![
            Node::Range(range),
            Node::Call {
                name: "SUM".into(),
                arguments: vec![NodeId(0)],
            },
        ],
    };
    assert!(matches!(
        evaluate(&ast, &NoReferences, &mut EvaluationBudget::default()),
        Err(EvaluationError::CalculationLimitExceeded)
    ));
    assert!(canonical(&ast).is_err());
    let ast = Ast {
        schema_version: 1,
        root: NodeId(0),
        nodes: vec![Node::Unary {
            op: UnaryOp::Plus,
            operand: NodeId(0),
        }],
    };
    assert!(matches!(
        evaluate(&ast, &NoReferences, &mut EvaluationBudget::default()),
        Err(EvaluationError::CalculationLimitExceeded)
    ));
    assert!(canonical(&ast).is_err());
    Ok(())
}

#[test]
fn diagnostics_use_safe_utf8_byte_spans() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "='日本語'!A0",
        "=\"ёж",
        "=A$",
        "=A1+*B2",
        "=SUM(1;2)",
        "=A1:B9999999",
        "=1e+",
        "='TRUE'",
        "='SUM'(1)",
        "=ФУНКЦИЯ(1)",
    ] {
        let error = parse(source)
            .err()
            .ok_or("invalid source unexpectedly parsed")?;
        assert!(error.span.start <= error.span.end && error.span.end <= source.len());
        assert!(
            source.is_char_boundary(error.span.start) && source.is_char_boundary(error.span.end)
        );
    }
    Ok(())
}

#[test]
fn seeded_hundred_thousand_parser_inputs_reach_lexer_without_panics() {
    let symbols = [
        'A', '1', '0', '(', ')', '+', '-', '*', '/', '^', ',', ':', '$', '!', '"', '\'', 'я', '日',
        '\0', '\n',
    ];
    let prefixes = [
        "=",
        "=SUM(",
        "=IF(TRUE,",
        "='Лист 1'!",
        "=A$1:",
        "=\"",
        "=1e",
        "=(((",
    ];
    let mut seed = 0x9e3779b97f4a7c15u64;
    for index in 0..100_000 {
        let mut input = prefixes[index % prefixes.len()].to_owned();
        for _ in 0..index % 64 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            input.push(symbols[(seed as usize) % symbols.len()]);
        }
        match parse(&input) {
            Ok(parsed) => {
                let _ = canonical(&parsed.ast);
                let _ = evaluate(&parsed.ast, &NoReferences, &mut EvaluationBudget::new(1000));
            }
            Err(error) => {
                assert!(error.span.start <= error.span.end && error.span.end <= input.len());
                assert!(
                    input.is_char_boundary(error.span.start)
                        && input.is_char_boundary(error.span.end)
                );
            }
        }
    }
}
