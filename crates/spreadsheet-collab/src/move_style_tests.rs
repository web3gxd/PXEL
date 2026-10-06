use super::*;
use spreadsheet_domain::{
    style::{Change, Rgb, StyleChange},
    workbook::{
        CellAddress, CellValue, ColumnId, DocumentId, OperationId, RowId, SheetId, SheetSeed,
    },
};

#[test]
fn range_move_transfers_all_sparse_style_leaves_in_one_candidate()
-> Result<(), Box<dyn std::error::Error>> {
    let sheet_id = SheetId::from_bytes([31; 16])?;
    let row = RowId::from_bytes([32; 16])?;
    let source_column = ColumnId::from_bytes([33; 16])?;
    let destination_column = ColumnId::from_bytes([34; 16])?;
    let seed = SheetSeed {
        id: sheet_id,
        name: "Sheet".into(),
        rows: vec![row],
        columns: vec![source_column, destination_column],
    };
    let book = Workbook::empty(DocumentId::from_bytes([35; 16])?).candidate(
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
            value: CellValue::Text("moving".into()),
        },
        DocumentVersion::new(2)?,
    )?;
    let book = book.candidate(
        &spreadsheet_domain::style_write::prepare_style_write(
            &book,
            &[source],
            &[
                StyleChange::FontBold(Change::Set(true)),
                StyleChange::Fill(Change::Set(Rgb::new([8, 19, 27]))),
            ],
            StyleWriteKind::Cell,
        )?,
        DocumentVersion::new(3)?,
    )?;
    let command = Command {
        operation_id: OperationId::from_bytes([36; 16])?,
        document_id: book.document_id(),
        actor_id: spreadsheet_domain::workbook::ActorId::from_bytes([37; 16])?,
        base_version: book.version(),
        client_sequence: 1,
        operation: CommandPayload::MoveRange {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    };
    let operation = normalize(&command, &book, &axis_history::AxisHistory::new(&book))?;
    let Operation::CellBatchWrite {
        intent: CellBatchIntent::MoveRange { .. },
        style_writes,
        ..
    } = &operation
    else {
        return Err("move did not normalize to an atomic styled batch".into());
    };
    assert_eq!(style_writes.len(), 2);
    assert!(style_writes.iter().all(|write| write.changes.len() == 16));
    let moved = book.candidate(&operation, DocumentVersion::new(4)?)?;
    assert_eq!(moved.cell_content(source)?, None);
    assert_eq!(
        moved.cell_content(destination)?,
        Some(&CellContent::Value(CellValue::Text("moving".into())))
    );
    assert_eq!(moved.sheets()[0].styles().get(&source), None);
    assert!(
        moved.sheets()[0]
            .styles()
            .get(&destination)
            .is_some_and(|style| style.font_bold())
    );
    let Operation::CellBatchWrite { content_writes, .. } = &operation else {
        return Err("move batch unexpectedly changed type".into());
    };
    let inverse = Operation::CellBatchWrite {
        intent: CellBatchIntent::MoveRange {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
        content_writes: content_writes
            .iter()
            .map(|write| spreadsheet_domain::workbook::GuardedContentWrite {
                address: write.address,
                content: write.previous.clone(),
                previous: write.content.clone(),
            })
            .collect(),
        style_writes: style_writes
            .iter()
            .map(|write| GuardedStyleWrite {
                address: write.address,
                previous: write.changes.clone(),
                changes: write.previous.clone(),
            })
            .collect(),
        undoes: Some(command.operation_id),
        undoes_version: Some(moved.version()),
    };
    let restored = moved.candidate(&inverse, DocumentVersion::new(5)?)?;
    assert_eq!(
        restored.cell_content(source)?,
        Some(&CellContent::Value(CellValue::Text("moving".into())))
    );
    assert_eq!(
        restored.sheets()[0].styles().get(&source),
        book.sheets()[0].styles().get(&source)
    );
    assert_eq!(
        restored.sheets()[0].styles().get(&destination),
        book.sheets()[0].styles().get(&destination)
    );
    Ok(())
}
