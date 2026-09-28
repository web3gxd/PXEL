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
    for function in ["SUM", "AVERAGE", "MIN", "MAX", "COUNT", "COUNTA"] {
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
    c.bench_function("parse_scalar_if", |b| {
        b.iter(|| black_box(parse(black_box("=IF(A1>0,SUM($B$1:B100),-2^2)"))))
    });
}
criterion_group!(scalar, benchmarks);
criterion_main!(scalar);
