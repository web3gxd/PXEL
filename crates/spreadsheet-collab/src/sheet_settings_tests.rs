use super::*;
use spreadsheet_domain::workbook::*;
#[test]
fn copy_and_settings_require_exact_current_head_before_freezing()
-> Result<(), Box<dyn std::error::Error>> {
    let seed = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Source".into(),
        rows: vec![RowId::from_bytes([3; 16])?],
        columns: vec![ColumnId::from_bytes([4; 16])?],
    };
    let initial = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let changed = initial.candidate(
        &Operation::SetCellValue {
            address: CellAddress {
                sheet: seed.id,
                row: seed.rows[0],
                column: seed.columns[0],
            },
            value: CellValue::Number(123.),
        },
        DocumentVersion::new(2)?,
    )?;
    let copy = SheetSeed {
        id: SheetId::from_bytes([5; 16])?,
        name: "Copy".into(),
        rows: vec![RowId::from_bytes([6; 16])?],
        columns: vec![ColumnId::from_bytes([7; 16])?],
    };
    for operation in [
        CommandPayload::CopySheet {
            source: seed.id,
            sheet: copy,
            index: 1,
        },
        CommandPayload::SetSheetSettings {
            sheet: seed.id,
            changes: SheetSettingsPatch {
                tab_color: Some(Some(0x123456)),
                ..Default::default()
            },
        },
    ] {
        let mut command = Command {
            document_id: changed.document_id(),
            operation_id: OperationId::from_bytes([8; 16])?,
            actor_id: ActorId::from_bytes([9; 16])?,
            client_sequence: 1,
            base_version: initial.version(),
            operation,
        };
        assert!(matches!(
            normalize(
                &command,
                &changed,
                &axis_history::AxisHistory::new(&changed)
            ),
            Err(SessionError::SourceVersionChanged)
        ));
        command.base_version = changed.version();
        let op = normalize(
            &command,
            &changed,
            &axis_history::AxisHistory::new(&changed),
        )?;
        assert!(changed.candidate(&op, DocumentVersion::new(3)?).is_ok());
    }
    Ok(())
}
