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
    for function in ["SUM", "COUNT", "COUNTA"] {
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
        Err(EvaluationError::UnsupportedCompatibilityFeature)
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
