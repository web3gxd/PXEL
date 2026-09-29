use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_domain::workbook::*;
use std::hint::black_box;

fn sheet_seed(row_count: usize) -> Result<SheetSeed, DomainError> {
    Ok(SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Sheet".into(),
        rows: (1..=row_count)
            .map(|n| RowId::from_bytes((n as u128).to_be_bytes()))
            .collect::<Result<Vec<_>, _>>()?,
        columns: vec![ColumnId::from_bytes([3; 16])?],
    })
}

fn book_with_axes(row_count: usize) -> Result<Workbook, DomainError> {
    Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: sheet_seed(row_count)?,
        },
        DocumentVersion::new(1)?,
    )
}

fn fixture(row_count: usize) -> Result<Workbook, DomainError> {
    let seed = sheet_seed(row_count)?;
    let cells = seed
        .rows
        .iter()
        .enumerate()
        .map(|(index, &row)| {
            (
                CellAddress {
                    sheet: seed.id,
                    row,
                    column: seed.columns[0],
                },
                CellValue::Number(index as f64),
            )
        })
        .collect::<Vec<_>>();
    Workbook::from_fixture(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        seed,
        cells,
    )
}

fn text_budget_fixture() -> Result<(Workbook, Operation, DocumentVersion), DomainError> {
    let count = MAX_WORKBOOK_TEXT_BYTES / MAX_TEXT_BYTES;
    let seed = sheet_seed(count + 1)?;
    let cells = seed.rows[..count]
        .iter()
        .map(|&row| {
            (
                CellAddress {
                    sheet: seed.id,
                    row,
                    column: seed.columns[0],
                },
                CellValue::Text("x".repeat(MAX_TEXT_BYTES)),
            )
        })
        .collect::<Vec<_>>();
    let rejected = Operation::SetCellValue {
        address: CellAddress {
            sheet: seed.id,
            row: seed.rows[count],
            column: seed.columns[0],
        },
        value: CellValue::Text("x".into()),
    };
    Ok((
        Workbook::from_fixture(
            DocumentId::from_bytes([1; 16])?,
            DocumentVersion::new(1)?,
            seed,
            cells,
        )?,
        rejected,
        DocumentVersion::new(2)?,
    ))
}

fn benches(c: &mut Criterion) {
    let mut axes = c.benchmark_group("axis_allocation");
    for count in [1_000, 100_000] {
        axes.bench_function(count.to_string(), |b| {
            b.iter(|| black_box(book_with_axes(black_box(count))))
        });
    }
    axes.finish();
    let insertion_book = match book_with_axes(99_999) {
        Ok(book) => book,
        Err(error) => {
            eprintln!("insertion benchmark fixture invalid: {error}");
            std::process::exit(1);
        }
    };
    let insertion_sheet = &insertion_book.sheets()[0];
    let Some(first_row) = insertion_sheet.rows().first().copied() else {
        eprintln!("insertion benchmark fixture has no rows");
        std::process::exit(1);
    };
    let insertion_row = match RowId::from_bytes((100_000u128).to_be_bytes()) {
        Ok(row) => row,
        Err(error) => {
            eprintln!("insertion benchmark row invalid: {error}");
            std::process::exit(1);
        }
    };
    let insertion = Operation::InsertRows {
        sheet: insertion_sheet.id(),
        before: first_row,
        rows: vec![insertion_row],
    };
    let insertion_version = match DocumentVersion::new(2) {
        Ok(version) => version,
        Err(error) => {
            eprintln!("insertion benchmark version invalid: {error}");
            std::process::exit(1);
        }
    };
    c.bench_function("insert_front_99999_rows", |b| {
        b.iter(|| black_box(insertion_book.candidate(black_box(&insertion), insertion_version)))
    });
    for count in [1_000, 100_000] {
        let book = match fixture(count) {
            Ok(book) => book,
            Err(error) => {
                eprintln!("benchmark fixture invalid: {error}");
                std::process::exit(1);
            }
        };
        c.bench_function(&format!("hash_{count}_sparse_cells"), |b| {
            b.iter(|| black_box(book.hash()))
        });
        let sheet = &book.sheets()[0];
        let operation = Operation::SetCellValue {
            address: CellAddress {
                sheet: sheet.id(),
                row: sheet.rows()[count / 2],
                column: sheet.columns()[0],
            },
            value: CellValue::Number(4.0),
        };
        let version = match DocumentVersion::new(2) {
            Ok(version) => version,
            Err(error) => {
                eprintln!("benchmark version invalid: {error}");
                std::process::exit(1);
            }
        };
        c.bench_function(&format!("candidate_{count}_sparse_cells"), |b| {
            b.iter(|| black_box(book.candidate(black_box(&operation), version)))
        });
    }
    let (book, operation, version) = match text_budget_fixture() {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("benchmark text fixture invalid: {error}");
            std::process::exit(1);
        }
    };
    if !matches!(
        book.candidate(&operation, version),
        Err(DomainError::LimitExceeded)
    ) {
        eprintln!("benchmark text budget rejection missing");
        std::process::exit(1);
    }
    c.bench_function("reject_16mib_workbook_text_budget", |b| {
        b.iter(|| black_box(book.candidate(black_box(&operation), version)))
    });
}
criterion_group!(workbook, benches);
criterion_main!(workbook);
