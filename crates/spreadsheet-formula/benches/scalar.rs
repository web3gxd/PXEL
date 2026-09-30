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
    if table_benchmarks(c).is_err() {
        eprintln!("invalid table lookup benchmark fixture");
        std::process::exit(1);
    }
    if index_benchmarks(c).is_err() {
        eprintln!("invalid INDEX benchmark fixture");
        std::process::exit(1);
    }
    if volatile_benchmarks(c).is_err() {
        eprintln!("invalid volatile benchmark fixture");
        std::process::exit(1);
    }
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
fn volatile_benchmarks(c: &mut Criterion) -> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::workbook::*;
    use std::collections::BTreeMap;
    let cell = CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: RowId::from_bytes([2; 16])?,
        column: ColumnId::from_bytes([5; 16])?,
    };
    let context = CalculationContext::new(
        CalculationContextId::from_bytes([6; 16])?,
        1,
        86400000,
        180,
        1,
        [7; 32],
        calculation::ENGINE_VERSION.into(),
        calculation::COMPATIBILITY_PROFILE.into(),
        CalculationReason::Explicit,
    )?;
    let book = Workbook::hydrate_sheets_with_context(
        DocumentId::from_bytes([3; 16])?,
        DocumentVersion::new(3)?,
        [SheetSeed {
            id: cell.sheet,
            name: "Sheet1".into(),
            rows: vec![cell.row],
            columns: vec![cell.column],
        }],
        [],
        context,
    )?;
    let cache = BTreeMap::new();
    for (name, source, expected) in [
        ("volatile_TODAY", "=TODAY()", 25570.0),
        ("volatile_NOW", "=NOW()", 25570.125),
        ("volatile_RAND", "=RAND()", 0.8435600626462038),
    ] {
        let formula = binder::bind(source, &book, cell)?;
        if bound::evaluate_bound(&formula, &book, &cache, &mut EvaluationBudget::default())?
            != Value::Number(expected)
        {
            return Err("invalid volatile expected result".into());
        }
        c.bench_function(name, |b| {
            b.iter(|| {
                black_box(bound::evaluate_bound(
                    black_box(&formula),
                    &book,
                    &cache,
                    &mut EvaluationBudget::default(),
                ))
            })
        });
    }
    Ok(())
}
criterion_group!(scalar, benchmarks);
criterion_main!(scalar);

fn table_benchmarks(c: &mut Criterion) -> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::workbook::*;
    use std::collections::BTreeMap;
    for (vertical, row_count, column_count, end) in [
        (true, 50000u32, 2u32, "B50000"),
        (false, 100, 1000, "ALL100"),
    ] {
        let sheet = SheetId::from_bytes([1; 16])?;
        let rows = (1..=row_count)
            .map(|n| RowId::from_bytes(u128::from(n).to_be_bytes()))
            .collect::<Result<Vec<_>, _>>()?;
        let columns = (1..=column_count)
            .map(|n| ColumnId::from_bytes(u128::from(n).to_be_bytes()))
            .collect::<Result<Vec<_>, _>>()?;
        let origin = CellAddress {
            sheet,
            row: rows[0],
            column: columns[0],
        };
        let mut cells = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (col, column) in columns.iter().enumerate() {
                let key = if vertical { col == 0 } else { r == 0 };
                cells.push((
                    CellAddress {
                        sheet,
                        row: *row,
                        column: *column,
                    },
                    CellContent::Value(CellValue::Number(if key { 1. } else { 2. })),
                ));
            }
        }
        let book = Workbook::from_content_fixture(
            DocumentId::from_bytes([3; 16])?,
            DocumentVersion::new(1)?,
            SheetSeed {
                id: sheet,
                name: "Sheet1".into(),
                rows,
                columns,
            },
            cells,
        )?;
        let cache = BTreeMap::new();
        for approximate in [false, true] {
            let function = if vertical { "VLOOKUP" } else { "HLOOKUP" };
            let mode = if approximate { "TRUE" } else { "FALSE" };
            let formula =
                binder::bind(&format!("={function}(1,A1:{end},2,{mode})"), &book, origin)?;
            if bound::evaluate_bound(&formula, &book, &cache, &mut EvaluationBudget::default())?
                != Value::Number(2.)
            {
                return Err("invalid table expected result".into());
            }
            c.bench_function(
                &format!("{function}_bound_table_{row_count}x{column_count}_{mode}"),
                |b| {
                    b.iter(|| {
                        black_box(bound::evaluate_bound(
                            black_box(&formula),
                            &book,
                            &cache,
                            &mut EvaluationBudget::default(),
                        ))
                    })
                },
            );
        }
    }
    Ok(())
}

fn index_benchmarks(c: &mut Criterion) -> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::workbook::*;
    use std::collections::BTreeMap;
    let sheet = SheetId::from_bytes([1; 16])?;
    let column = ColumnId::from_bytes([2; 16])?;
    let rows = (1..=100000u128)
        .map(|n| RowId::from_bytes(n.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let origin = CellAddress {
        sheet,
        row: rows[0],
        column,
    };
    let cells = rows
        .iter()
        .map(|row| {
            (
                CellAddress {
                    sheet,
                    row: *row,
                    column,
                },
                CellContent::Value(CellValue::Number(1.0)),
            )
        })
        .collect::<Vec<_>>();
    let book = Workbook::from_content_fixture(
        DocumentId::from_bytes([3; 16])?,
        DocumentVersion::new(1)?,
        SheetSeed {
            id: sheet,
            name: "Sheet1".into(),
            rows,
            columns: vec![column],
        },
        cells,
    )?;
    let cache = BTreeMap::new();
    for (name, source, expected) in [
        (
            "INDEX_bound_select_from_100k",
            "=INDEX(A1:A100000,100000)",
            1.0,
        ),
        (
            "INDEX_bound_SUM_100k",
            "=SUM(INDEX(A1:A100000,0))",
            100000.0,
        ),
        (
            "MATCH_bound_exact_first_100k",
            "=MATCH(1,A1:A100000,0)",
            1.0,
        ),
        (
            "XLOOKUP_bound_forward_100k",
            "=XLOOKUP(1,A1:A100000,A1:A100000)",
            1.0,
        ),
        (
            "XLOOKUP_bound_reverse_100k",
            "=XLOOKUP(1,A1:A100000,A1:A100000,,0,-1)",
            1.0,
        ),
        (
            "XLOOKUP_bound_binary_ascending_100k",
            "=XLOOKUP(1,A1:A100000,A1:A100000,,0,2)",
            1.0,
        ),
        (
            "XLOOKUP_bound_binary_descending_100k",
            "=XLOOKUP(1,A1:A100000,A1:A100000,,0,-2)",
            1.0,
        ),
        (
            "MATCH_bound_approx_last_100k",
            "=MATCH(1,A1:A100000,1)",
            100000.0,
        ),
        (
            "COUNTIF_bound_100k",
            "=COUNTIF(A1:A100000,\">0\")",
            100000.0,
        ),
        ("SUMIF_bound_100k", "=SUMIF(A1:A100000,\">0\")", 100000.0),
        (
            "SUMIF_bound_anchor_100k",
            "=SUMIF(A1:A100000,\">0\",A1)",
            100000.0,
        ),
    ] {
        let formula = binder::bind(source, &book, origin)?;
        if bound::evaluate_bound(&formula, &book, &cache, &mut EvaluationBudget::default())?
            != Value::Number(expected)
        {
            return Err("invalid INDEX result".into());
        }
        c.bench_function(name, |b| {
            b.iter(|| {
                black_box(bound::evaluate_bound(
                    black_box(&formula),
                    &book,
                    &cache,
                    &mut EvaluationBudget::default(),
                ))
            })
        });
    }
    Ok(())
}
