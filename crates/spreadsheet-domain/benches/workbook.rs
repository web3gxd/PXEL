use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_domain::formula::{
    AST_SCHEMA_VERSION, AxisReferenceMode, BoundAst, BoundCellReference, Formula, Node, NodeId,
};
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

fn formula_insertion_fixture(count: usize) -> Result<(Workbook, Operation), DomainError> {
    let seed = sheet_seed(count + 1)?;
    let version = DocumentVersion::new(1)?;
    let cells = seed
        .rows
        .iter()
        .enumerate()
        .take(count)
        .map(|(index, &row)| {
            let address = CellAddress {
                sheet: seed.id,
                row,
                column: seed.columns[0],
            };
            let reference = BoundCellReference {
                target_sheet: seed.id,
                target_row: seed.rows[0],
                target_column: seed.columns[0],
                row_mode: AxisReferenceMode::Relative {
                    original_delta: -(index as i64),
                },
                column_mode: AxisReferenceMode::Relative { original_delta: 0 },
            };
            let ast = BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Cell(reference)])
                .map_err(|_| DomainError::InvalidFormula)?;
            let formula = Formula::new("=A1".into(), "=A1".into(), ast, address, version)
                .map_err(|_| DomainError::InvalidFormula)?;
            Ok((address, CellContent::Formula(formula)))
        })
        .collect::<Result<Vec<_>, DomainError>>()?;
    let operation = Operation::InsertRowsRebased {
        sheet: seed.id,
        before: seed.rows[count / 2],
        rows: vec![RowId::from_bytes(((count + 2) as u128).to_be_bytes())?],
    };
    let book =
        Workbook::from_content_fixture(DocumentId::from_bytes([1; 16])?, version, seed, cells)?;
    Ok((book, operation))
}

fn formula_column_deletion_fixture(count: usize) -> Result<(Workbook, Operation), DomainError> {
    let mut seed = sheet_seed(count)?;
    let source_column = seed.columns[0];
    let formula_column = ColumnId::from_bytes([4; 16])?;
    seed.columns.push(formula_column);
    let version = DocumentVersion::new(1)?;
    let cells = seed
        .rows
        .iter()
        .enumerate()
        .map(|(index, &row)| {
            let address = CellAddress {
                sheet: seed.id,
                row,
                column: formula_column,
            };
            let reference = BoundCellReference {
                target_sheet: seed.id,
                target_row: seed.rows[0],
                target_column: source_column,
                row_mode: AxisReferenceMode::Relative {
                    original_delta: -(index as i64),
                },
                column_mode: AxisReferenceMode::Relative { original_delta: -1 },
            };
            let ast = BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Cell(reference)])
                .map_err(|_| DomainError::InvalidFormula)?;
            let formula = Formula::new("=A1".into(), "=A1".into(), ast, address, version)
                .map_err(|_| DomainError::InvalidFormula)?;
            Ok((address, CellContent::Formula(formula)))
        })
        .collect::<Result<Vec<_>, DomainError>>()?;
    let operation = Operation::DeleteColumns {
        sheet: seed.id,
        columns: vec![source_column],
    };
    let book =
        Workbook::from_content_fixture(DocumentId::from_bytes([1; 16])?, version, seed, cells)?;
    Ok((book, operation))
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
    let (formula_book, formula_insertion) = match formula_insertion_fixture(10_000) {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("formula insertion benchmark fixture invalid: {error}");
            std::process::exit(1);
        }
    };
    c.bench_function("insert_middle_10000_formulas", |b| {
        b.iter(|| {
            black_box(formula_book.candidate(black_box(&formula_insertion), insertion_version))
        })
    });
    let deleted_row = formula_book.sheets()[0].rows()[0];
    let formula_deletion = Operation::DeleteRows {
        sheet: formula_book.sheets()[0].id(),
        rows: vec![deleted_row],
    };
    c.bench_function("delete_target_10000_formulas", |b| {
        b.iter(|| {
            black_box(formula_book.candidate(black_box(&formula_deletion), insertion_version))
        })
    });
    let formula_column_insertion = Operation::InsertColumnsRebased {
        sheet: formula_book.sheets()[0].id(),
        before: formula_book.sheets()[0].columns()[0],
        columns: vec![match ColumnId::from_bytes([4; 16]) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("column benchmark ID invalid: {error}");
                std::process::exit(1);
            }
        }],
    };
    c.bench_function("insert_front_column_10000_formulas", |b| {
        b.iter(|| {
            black_box(
                formula_book.candidate(black_box(&formula_column_insertion), insertion_version),
            )
        })
    });
    let (column_deletion_book, column_deletion) = match formula_column_deletion_fixture(10_000) {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("column deletion benchmark fixture invalid: {error}");
            std::process::exit(1);
        }
    };
    c.bench_function("delete_target_column_10000_formulas", |b| {
        b.iter(|| {
            black_box(
                column_deletion_book.candidate(black_box(&column_deletion), insertion_version),
            )
        })
    });
    let batch_book = match book_with_axes(1_000) {
        Ok(book) => book,
        Err(error) => {
            eprintln!("batch fixture invalid: {error}");
            std::process::exit(1);
        }
    };
    let batch_sheet = &batch_book.sheets()[0];
    let batch = Operation::SetCellValues {
        writes: batch_sheet
            .rows()
            .iter()
            .map(|&row| CellWrite {
                address: CellAddress {
                    sheet: batch_sheet.id(),
                    row,
                    column: batch_sheet.columns()[0],
                },
                value: CellValue::Number(42.0),
            })
            .collect(),
    };
    c.bench_function("candidate_1000_value_batch", |b| {
        b.iter(|| black_box(batch_book.candidate(black_box(&batch), insertion_version)))
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
