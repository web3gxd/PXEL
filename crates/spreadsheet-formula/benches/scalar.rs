use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_formula::*;
use std::hint::black_box;
struct Range;
impl ReferenceResolver for Range {
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
fn benchmarks(c: &mut Criterion) {
    for (name, source, expected) in [
        ("ABS_scalar", "=ABS(-123.5)", Value::Number(123.5)),
        ("DATE_scalar", "=DATE(2000,1,1)", Value::Number(36526.0)),
        (
            "DATE_parts",
            "=YEAR(36585)*10000+MONTH(36585)*100+DAY(36585)",
            Value::Number(20000229.0),
        ),
        (
            "TEXT_scalar",
            "=CONCAT(UPPER(LEFT(TRIM(\"  абв  \"),2)),LOWER(RIGHT(\"Я\",1)),MID(\"😀abc\",2,2),LEN(\"😀\"))",
            Value::Text("АБяab1".into()),
        ),
        ("POWER_scalar", "=POWER(4,0.5)", Value::Number(2.0)),
        ("MOD_scalar", "=MOD(-123.5,7)", Value::Number(2.5)),
        ("ROUND_scalar", "=ROUND(1.005,2)", Value::Number(1.01)),
        (
            "ROUNDUP_scalar",
            "=ROUNDUP(-12.34567,3)",
            Value::Number(-12.346),
        ),
        (
            "ROUNDDOWN_scalar",
            "=ROUNDDOWN(-12.34567,3)",
            Value::Number(-12.345),
        ),
    ] {
        let parsed = match parse(source) {
            Ok(parsed) => parsed,
            Err(_) => {
                eprintln!("invalid math benchmark syntax");
                std::process::exit(1);
            }
        };
        if evaluate(&parsed.ast, &NoReferences, &mut EvaluationBudget::default()) != Ok(expected) {
            eprintln!("invalid math benchmark result");
            std::process::exit(1);
        }
        c.bench_function(name, |b| {
            b.iter(|| {
                black_box(evaluate(
                    black_box(&parsed.ast),
                    &NoReferences,
                    &mut EvaluationBudget::default(),
                ))
            })
        });
    }
    for function in [
        "SUM", "AVERAGE", "MIN", "MAX", "COUNT", "COUNTA", "AND", "OR",
    ] {
        let parsed = match parse(&format!("={function}(A1:A100000)")) {
            Ok(value) => value,
            Err(_) => {
                eprintln!("invalid benchmark syntax");
                std::process::exit(1);
            }
        };
        if evaluate(&parsed.ast, &Range, &mut EvaluationBudget::default()).is_err() {
            eprintln!("invalid benchmark evaluation");
            std::process::exit(1);
        }
        if matches!(function, "AND" | "OR")
            && evaluate(&parsed.ast, &Range, &mut EvaluationBudget::default())
                != Ok(Value::Boolean(true))
        {
            eprintln!("invalid logical benchmark result");
            std::process::exit(1);
        }
        c.bench_function(&format!("{function}_range_100k"), |b| {
            b.iter(|| {
                black_box(evaluate(
                    black_box(&parsed.ast),
                    &Range,
                    &mut EvaluationBudget::default(),
                ))
            })
        });
    }
    let parsed = match parse("=CONCAT(A1:A10000)") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("invalid text range benchmark");
            std::process::exit(1);
        }
    };
    if evaluate(&parsed.ast, &Range, &mut EvaluationBudget::default())
        != Ok(Value::Text("1".repeat(10000)))
    {
        eprintln!("invalid text range result");
        std::process::exit(1);
    }
    c.bench_function("CONCAT_range_10k", |b| {
        b.iter(|| {
            black_box(evaluate(
                black_box(&parsed.ast),
                &Range,
                &mut EvaluationBudget::default(),
            ))
        })
    });
    c.bench_function("parse_scalar_if", |b| {
        b.iter(|| black_box(parse(black_box("=IF(A1>0,SUM($B$1:B100),-2^2)"))))
    });
}
criterion_group!(scalar, benchmarks);
criterion_main!(scalar);
