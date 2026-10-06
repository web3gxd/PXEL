use super::*;
use spreadsheet_domain::{
    style::{Change, HorizontalAlignment, Rgb, StyleChange},
    workbook::{
        CellAddress, CellValue, ColumnId, DocumentId, OperationId, RowId, SheetId, SheetSeed,
    },
};

#[test]
fn range_copy_freezes_values_and_replaces_full_destination_style_atomically()
-> Result<(), Box<dyn std::error::Error>> {
    let sheet_id = SheetId::from_bytes([1; 16])?;
    let row = RowId::from_bytes([2; 16])?;
    let source_column = ColumnId::from_bytes([3; 16])?;
    let destination_column = ColumnId::from_bytes([4; 16])?;
    let seed = SheetSeed {
        id: sheet_id,
        name: "Sheet".into(),
        rows: vec![row],
        columns: vec![source_column, destination_column],
    };
    let book = Workbook::empty(DocumentId::from_bytes([5; 16])?).candidate(
        &Operation::CreateWorkbook { sheet: seed },
        DocumentVersion::new(1)?,
    )?;
    let source = CellAddress {
        sheet: sheet_id,
        row,
        column: source_column,
    };
    let destination = CellAddress {
        sheet: sheet_id,
        row,
        column: destination_column,
    };
    let book = book.candidate(
        &Operation::SetCellValue {
            address: source,
            value: CellValue::Text("source".into()),
        },
        DocumentVersion::new(2)?,
    )?;
    let book = book.candidate(
        &Operation::SetCellValue {
            address: destination,
            value: CellValue::Text("old target".into()),
        },
        DocumentVersion::new(3)?,
    )?;
    let book = book.candidate(
        &spreadsheet_domain::style_write::prepare_style_write(
            &book,
            &[source],
            &[
                StyleChange::Horizontal(Change::Set(HorizontalAlignment::Center)),
                StyleChange::FontBold(Change::Set(true)),
            ],
            StyleWriteKind::Cell,
        )?,
        DocumentVersion::new(4)?,
    )?;
    let book = book.candidate(
        &spreadsheet_domain::style_write::prepare_style_write(
            &book,
            &[destination],
            &[StyleChange::Fill(Change::Set(Rgb::new([12, 34, 56])))],
            StyleWriteKind::Cell,
        )?,
        DocumentVersion::new(5)?,
    )?;
    let command = Command {
        operation_id: OperationId::from_bytes([6; 16])?,
        document_id: book.document_id(),
        actor_id: spreadsheet_domain::workbook::ActorId::from_bytes([7; 16])?,
        base_version: book.version(),
        client_sequence: 1,
        operation: CommandPayload::CopyRange {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    };
    let operation = normalize(&command, &book, &axis_history::AxisHistory::new(&book))?;
    let Operation::CellBatchWrite {
        intent: CellBatchIntent::CopyRange { .. },
        style_writes,
        ..
    } = &operation
    else {
        return Err("copy did not normalize to an atomic styled batch".into());
    };
    assert_eq!(style_writes.len(), 1);
    assert_eq!(style_writes[0].changes.len(), 16);
    let copied = book.candidate(&operation, DocumentVersion::new(6)?)?;
    assert_eq!(
        copied.cell_content(destination)?,
        copied.cell_content(source)?
    );
    assert_eq!(
        copied.sheets()[0].styles().get(&destination),
        copied.sheets()[0].styles().get(&source)
    );
    Ok(())
}
