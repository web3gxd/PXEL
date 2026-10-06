use super::*;
use spreadsheet_domain::workbook::{
    ActorId, CellAddress, CellValue, ColumnId, RowId, SheetId, SheetSeed,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture() -> Result<(Workbook, Command), Box<dyn std::error::Error>> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let rows = (2..8)
        .map(|n| RowId::from_bytes([n; 16]))
        .collect::<Result<Vec<_>, _>>()?;
    let columns = (10..14)
        .map(|n| ColumnId::from_bytes([n; 16]))
        .collect::<Result<Vec<_>, _>>()?;
    let document_id = DocumentId::from_bytes([20; 16])?;
    let mut book = Workbook::empty(document_id).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet1".into(),
                rows: rows.clone(),
                columns: columns.clone(),
            },
        },
        DocumentVersion::new(1)?,
    )?;
    let source = CellAddress {
        sheet,
        row: rows[0],
        column: columns[0],
    };
    book = book.candidate(
        &Operation::SetCellValue {
            address: source,
            value: CellValue::Number(7.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let formula_address = CellAddress {
        column: columns[1],
        ..source
    };
    let formula = spreadsheet_formula::binder::bind("=A1+$A$1", &book, formula_address)?;
    book = book.candidate(
        &Operation::SetCellFormula {
            address: formula_address,
            formula,
        },
        DocumentVersion::new(3)?,
    )?;
    let command = Command {
        operation_id: OperationId::from_bytes([21; 16])?,
        document_id,
        actor_id: ActorId::from_bytes([22; 16])?,
        base_version: book.version(),
        client_sequence: 1,
        operation: CommandPayload::CopyRange {
            source,
            destination: CellAddress {
                row: rows[1],
                ..source
            },
            rows: 1,
            columns: 2,
            repeat: Some((4, 2)),
        },
    };
    Ok((book, command))
}

#[test]
fn fills_from_frozen_top_row_and_shifts_relative_references() -> TestResult {
    let (book, command) = fixture()?;
    let operation = normalize(&command, &book, &axis_history::AxisHistory::new(&book))?;
    let Operation::RangeWrite { writes, .. } = &operation else {
        return Err("expected range write".into());
    };
    assert_eq!(writes.len(), 8);
    let candidate = book.candidate(&operation, DocumentVersion::new(4)?)?;
    for row in 0..4 {
        assert_eq!(
            writes[row * 2].content,
            Some(CellContent::Value(CellValue::Number(7.0)))
        );
        let Some(CellContent::Formula(formula)) = &writes[row * 2 + 1].content else {
            return Err("expected formula".into());
        };
        assert_eq!(
            spreadsheet_formula::bound::render_current(formula, &candidate)?,
            format!("=(A{}+$A$1)", row + 2)
        );
    }
    assert_eq!(book.version().get(), 3);
    Ok(())
}

#[test]
fn rejects_stale_invalid_and_overflowing_repetition_before_candidate() -> TestResult {
    let (book, command) = fixture()?;
    let axes = axis_history::AxisHistory::new(&book);
    for repeat in [
        Some((0, 2)),
        Some((4, 3)),
        Some((1001, 2)),
        Some((u32::MAX, 2)),
        Some((6, 2)),
    ] {
        let mut invalid = command.clone();
        if let CommandPayload::CopyRange { repeat: extent, .. } = &mut invalid.operation {
            *extent = repeat;
        }
        assert!(normalize(&invalid, &book, &axes).is_err());
    }
    let mut stale = command;
    stale.base_version = DocumentVersion::new(2)?;
    assert!(matches!(
        normalize(&stale, &book, &axes),
        Err(SessionError::SourceVersionChanged)
    ));
    Ok(())
}

#[test]
fn overlapping_tiles_read_original_source_instead_of_previous_writes() -> TestResult {
    let (mut book, mut command) = fixture()?;
    let sheet = &book.sheets()[0];
    let second = CellAddress {
        sheet: sheet.id(),
        row: sheet.rows()[1],
        column: sheet.columns()[0],
    };
    book = book.candidate(
        &Operation::SetCellValue {
            address: second,
            value: CellValue::Number(99.0),
        },
        DocumentVersion::new(4)?,
    )?;
    command.base_version = book.version();
    if let CommandPayload::CopyRange { rows, .. } = &mut command.operation {
        *rows = 2;
    }
    let operation = normalize(&command, &book, &axis_history::AxisHistory::new(&book))?;
    let Operation::RangeWrite { writes, .. } = &operation else {
        return Err("expected range write".into());
    };
    for (row, expected) in [7.0, 99.0, 7.0, 99.0].iter().enumerate() {
        assert_eq!(
            writes[row * 2].content,
            Some(CellContent::Value(CellValue::Number(*expected)))
        );
    }
    assert!(writes[3].content.is_none());
    assert!(writes[7].content.is_none());
    book.candidate(&operation, DocumentVersion::new(5)?)?;
    Ok(())
}
