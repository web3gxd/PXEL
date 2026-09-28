use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_domain::{formula as f, workbook::*};
use spreadsheet_formula::calculation::{CalculationLimits, CalculationProjection};
use std::{hint::black_box, sync::atomic::AtomicBool};

fn address(row: u32) -> Result<CellAddress, DomainError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: RowId::from_bytes((u128::from(row) + 10).to_be_bytes())?,
        column: ColumnId::from_bytes([3; 16])?,
    })
}
fn setup(c: &mut Criterion) -> Result<(), Box<dyn std::error::Error>> {
    let count = 10000;
    let seed = SheetSeed {
        id: SheetId::from_bytes([1; 16])?,
        name: "Sheet1".into(),
        rows: (0..=count)
            .map(|r| address(r).map(|a| a.row))
            .collect::<Result<_, _>>()?,
        columns: vec![ColumnId::from_bytes([3; 16])?],
    };
    let mut cells = vec![(address(0)?, CellContent::Value(CellValue::Number(1.0)))];
    for row in 1..=count {
        let target = address(row - 1)?;
        let ast = f::BoundAst::new(
            1,
            f::NodeId(2),
            vec![
                f::Node::Cell(f::BoundCellReference {
                    target_sheet: target.sheet,
                    target_row: target.row,
                    target_column: target.column,
                    row_mode: f::AxisReferenceMode::Absolute,
                    column_mode: f::AxisReferenceMode::Absolute,
                }),
                f::Node::Number(1.0),
                f::Node::Binary {
                    op: f::BinaryOp::Add,
                    left: f::NodeId(0),
                    right: f::NodeId(1),
                },
            ],
        )?;
        let formula = f::Formula::new(
            "=reference+1".into(),
            "=reference+1".into(),
            ast,
            address(row)?,
            DocumentVersion::new(1)?,
        )?;
        cells.push((address(row)?, CellContent::Formula(formula)));
    }
    let book = Workbook::from_content_fixture(
        DocumentId::from_bytes([2; 16])?,
        DocumentVersion::new(1)?,
        seed,
        cells,
    )?;
    c.bench_function("calculate_10000_bound_formula_chain", |b| {
        b.iter_batched(
            || CalculationProjection::from_workbook(&book, CalculationLimits::default()),
            |projection| {
                let result = projection.and_then(|mut p| {
                    p.calculate(&book, CalculationLimits::default(), &AtomicBool::new(false))
                });
                assert!(
                    result
                        .as_ref()
                        .is_ok_and(|patch| patch.evaluated_cells == 10000),
                    "real calculation failed"
                );
                black_box(result)
            },
            criterion::BatchSize::SmallInput,
        )
    });
    Ok(())
}
fn measure(c: &mut Criterion) {
    assert!(setup(c).is_ok(), "calculation benchmark setup failed");
}
criterion_group!(benches, measure);
criterion_main!(benches);
