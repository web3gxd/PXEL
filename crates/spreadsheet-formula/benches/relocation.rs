use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_domain::workbook::*;
use spreadsheet_formula::{
    binder::bind,
    relocation::{MoveRequest, plan_move},
};
use std::hint::black_box;

fn setup(c: &mut Criterion) -> Result<(), Box<dyn std::error::Error>> {
    let sheet = SheetId::from_bytes([2; 16])?;
    let rows = (1u128..=6000)
        .map(|n| RowId::from_bytes(n.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let columns = (10001u128..=10003)
        .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let seed = SheetSeed {
        id: sheet,
        name: "Sheet".into(),
        rows: rows.clone(),
        columns: columns.clone(),
    };
    let empty = Workbook::hydrate_sheets(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        [seed.clone()],
        [],
    )?;
    let mut cells = Vec::new();
    for (i, &row) in rows.iter().take(1000).enumerate() {
        cells.push((
            CellAddress {
                sheet,
                row,
                column: columns[0],
            },
            CellContent::Value(CellValue::Number(i as f64)),
        ));
    }
    for &row in rows.iter().take(5000) {
        let address = CellAddress {
            sheet,
            row,
            column: columns[2],
        };
        cells.push((
            address,
            CellContent::Formula(bind("=$A$1", &empty, address)?),
        ));
    }
    let book =
        Workbook::hydrate_sheets(empty.document_id(), DocumentVersion::new(2)?, [seed], cells)?;
    let request = MoveRequest {
        source: CellAddress {
            sheet,
            row: rows[0],
            column: columns[0],
        },
        destination: CellAddress {
            sheet,
            row: rows[0],
            column: columns[1],
        },
        rows: 1000,
        columns: 1,
    };
    assert_eq!(plan_move(&book, request)?.writes.len(), 7000);
    c.bench_function("move_1000_cells_5000_inbound_formulas", |b| {
        b.iter(|| black_box(plan_move(black_box(&book), request)))
    });
    let independent = book.sheets()[0]
        .cells()
        .iter()
        .map(|(&address, content)| {
            Ok((
                address,
                match content {
                    CellContent::Formula(_) => CellContent::Formula(bind("=42", &empty, address)?),
                    other => other.clone(),
                },
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let independent_book = Workbook::hydrate_sheets(
        empty.document_id(),
        book.version(),
        [SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows,
            columns,
        }],
        independent,
    )?;
    assert_eq!(plan_move(&independent_book, request)?.writes.len(), 2000);
    c.bench_function("move_1000_cells_5000_independent_formulas", |b| {
        b.iter(|| black_box(plan_move(black_box(&independent_book), request)))
    });
    Ok(())
}
fn measure(c: &mut Criterion) {
    assert!(setup(c).is_ok(), "relocation benchmark setup failed");
}
criterion_group!(benches, measure);
criterion_main!(benches);
