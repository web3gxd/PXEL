use super::*;
use spreadsheet_domain::workbook::SheetSettingsPatch;
#[test]
fn metadata_and_copy_events_snapshot_roundtrip_and_fingerprints_bind_intent()
-> Result<(), Box<dyn std::error::Error>> {
    let seed = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Sheet".into(),
        rows: vec![RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?],
        columns: vec![ColumnId::from_bytes([5; 16])?],
    };
    let actor = ActorId::from_bytes([6; 16])?;
    let document = DocumentId::from_bytes([1; 16])?;
    let book = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let patch = SheetSettingsPatch {
        default_column_width: Some(160),
        tab_color: Some(Some(0x123456)),
        visibility: vec![(
            spreadsheet_domain::workbook::AxisSizeTarget::Row(seed.rows[1]),
            true,
        )],
        ..Default::default()
    };
    let op = spreadsheet_domain::sheet_settings::prepare(&book, seed.id, &patch)?;
    let next = book.candidate(&op, DocumentVersion::new(2)?)?;
    let event = CommittedEvent {
        document_id: document,
        operation_id: OperationId::from_bytes([7; 16])?,
        principal: EventPrincipal::User(actor),
        version: next.version(),
        operation: op,
        state_hash: next.hash(),
    };
    assert_eq!(schema_version(&event.operation), 39);
    assert_eq!(decode(&encode(&event)?)?, event);
    let bytes = encode_snapshot(&next, "pxel-test-1", "pxel-excel-subset-v1")?;
    let restored = decode_snapshot(
        &bytes,
        document,
        next.version(),
        next.last_axis_change_version(),
        next.hash(),
        "pxel-test-1",
        "pxel-excel-subset-v1",
    )?;
    assert_eq!(restored.hash(), next.hash());
    assert_eq!(snapshot_schema_version(&bytes)?, 8);
    let command = Command {
        document_id: document,
        operation_id: event.operation_id,
        actor_id: actor,
        base_version: book.version(),
        client_sequence: 1,
        operation: CommandPayload::SetSheetSettings {
            sheet: seed.id,
            changes: patch.clone(),
        },
    };
    let hash = command_fingerprint(&command)?;
    let mut changed = command.clone();
    if let CommandPayload::SetSheetSettings { changes, .. } = &mut changed.operation {
        changes.tab_color = Some(None);
    }
    assert_ne!(hash, command_fingerprint(&changed)?);
    let copyseed = SheetSeed {
        id: SheetId::from_bytes([8; 16])?,
        name: "Copy".into(),
        rows: vec![RowId::from_bytes([9; 16])?, RowId::from_bytes([10; 16])?],
        columns: vec![ColumnId::from_bytes([11; 16])?],
    };
    let operation = Operation::CopySheet {
        source: seed.id,
        sheet: copyseed,
        index: 1,
    };
    let copied = next.candidate(&operation, DocumentVersion::new(3)?)?;
    let event = CommittedEvent {
        version: copied.version(),
        operation,
        state_hash: copied.hash(),
        ..event
    };
    assert_eq!(schema_version(&event.operation), 40);
    assert_eq!(decode(&encode(&event)?)?, event);
    Ok(())
}
