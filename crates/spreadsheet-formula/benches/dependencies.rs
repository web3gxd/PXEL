use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_domain::workbook::{CellAddress, ColumnId, RowId, SheetId};
use spreadsheet_formula::{
    dependency::DependencyGraph,
    range_index::{RangeDependency, RangeIndex},
};
use std::{collections::BTreeSet, hint::black_box};

fn measure(c: &mut Criterion) {
    let result = setup(c);
    assert!(result.is_ok(), "benchmark setup or traversal failed");
}

fn setup(c: &mut Criterion) -> Result<(), Box<dyn std::error::Error>> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let column = ColumnId::from_bytes([2; 16])?;
    let cells: Vec<_> = (1_u128..=10_001)
        .map(|row| {
            Ok(CellAddress {
                sheet,
                column,
                row: RowId::from_bytes(row.to_be_bytes())?,
            })
        })
        .collect::<Result<_, spreadsheet_domain::workbook::DomainError>>()?;
    let mut graph = DependencyGraph::default();
    for pair in cells.windows(2) {
        graph.replace(pair[1], BTreeSet::from([pair[0]]));
    }
    c.bench_function("dirty_and_schedule_10000_chain", |b| {
        b.iter(|| {
            let mut budget = 100_000;
            let dirty = graph.dirty([cells[0]], &mut budget);
            black_box(dirty.and_then(|dirty| graph.schedule(&dirty, &mut budget)))
        })
    });
    let ranges: Vec<_> = (0_u32..100_000)
        .map(|row| RangeDependency {
            formula: cells[0],
            sheet,
            first_row: row * 2,
            last_row: row * 2,
            first_column: 0,
            last_column: 1,
        })
        .collect();
    let index = RangeIndex::build(ranges.iter().copied())?;
    assert_eq!(index.affected(sheet, 1000, 0, &mut 50)?.len(), 1);
    c.bench_function("range_lookup_100000_disjoint", |b| {
        b.iter(|| black_box(index.affected(sheet, black_box(1000), black_box(0), &mut 50)))
    });
    c.bench_function("range_build_100000_disjoint", |b| {
        b.iter(|| black_box(RangeIndex::build(ranges.iter().copied())))
    });
    Ok(())
}

criterion_group!(benches, measure);
criterion_main!(benches);
