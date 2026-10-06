use crate::{decode, decode_snapshot, encode, encode_snapshot, v1};
use prost::Message;
use spreadsheet_domain::workbook::*;

#[test]
fn frozen_event_and_snapshot_have_independent_versions_and_keep_legacy_readers()
-> Result<(), Box<dyn std::error::Error>> {
    let doc = DocumentId::from_bytes([1; 16])?;
    let seed = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Лист".into(),
        rows: vec![RowId::from_bytes([3; 16])?],
        columns: vec![ColumnId::from_bytes([4; 16])?],
    };
    let book = Workbook::empty(doc).candidate(
        &Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let operation = Operation::FrozenRowsWrite {
        sheet: seed.id,
        rows: 1,
        previous: 0,
    };
    let frozen = book.candidate(&operation, DocumentVersion::new(2)?)?;
    let event = CommittedEvent {
        document_id: doc,
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: frozen.version(),
        operation,
        state_hash: frozen.hash(),
    };
    let encoded = encode(&event)?;
    assert_eq!(decode(&encoded)?, event);
    let mut envelope = v1::EventEnvelope::decode(encoded.as_slice())?;
    assert_eq!(envelope.schema_version, 36);
    envelope.schema_version = 35;
    assert!(decode(&envelope.encode_to_vec()).is_err());
    let bytes = encode_snapshot(&frozen, "engine-v1", "pxel-v1")?;
    let recovered = decode_snapshot(
        &bytes,
        doc,
        frozen.version(),
        frozen.last_axis_change_version(),
        frozen.hash(),
        "engine-v1",
        "pxel-v1",
    )?;
    assert_eq!(recovered.sheets()[0].frozen_rows(), 1);
    assert_eq!(recovered.hash(), frozen.hash());
    let mut raw = v1::SnapshotEnvelope::decode(bytes.as_slice())?;
    assert_eq!(raw.snapshot_schema_version, 7);
    raw.snapshot_schema_version = 6;
    assert!(
        decode_snapshot(
            &raw.encode_to_vec(),
            doc,
            frozen.version(),
            frozen.last_axis_change_version(),
            frozen.hash(),
            "engine-v1",
            "pxel-v1"
        )
        .is_err()
    );
    let mut legacy =
        v1::SnapshotEnvelope::decode(encode_snapshot(&book, "engine-v1", "pxel-v1")?.as_slice())?;
    legacy.snapshot_schema_version = 6;
    assert_eq!(
        decode_snapshot(
            &legacy.encode_to_vec(),
            doc,
            book.version(),
            book.last_axis_change_version(),
            book.hash(),
            "engine-v1",
            "pxel-v1"
        )?
        .hash(),
        book.hash()
    );
    Ok(())
}
