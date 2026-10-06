use crate::{decode, encode, encode_snapshot, v1};
use prost::Message;
use spreadsheet_domain::{
    merge::{MergeChange, MergeRange},
    workbook::{
        ActorId, ColumnId, CommittedEvent, DocumentId, DocumentVersion, DomainError,
        EventPrincipal, Operation, OperationId, RowId, SheetId, SheetSeed, Workbook,
    },
};

fn event(operation: Operation, version: u64) -> Result<CommittedEvent, Box<dyn std::error::Error>> {
    Ok(CommittedEvent {
        document_id: DocumentId::from_bytes([1; 16])?,
        operation_id: OperationId::from_bytes([10 + version as u8; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([9; 16])?),
        version: DocumentVersion::new(version)?,
        operation,
        state_hash: [7; 32],
    })
}

fn fixture() -> Result<(Workbook, MergeRange), Box<dyn std::error::Error>> {
    let document = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let rows = [RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?];
    let columns = [
        ColumnId::from_bytes([5; 16])?,
        ColumnId::from_bytes([6; 16])?,
    ];
    let book = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet".into(),
                rows: rows.to_vec(),
                columns: columns.to_vec(),
            },
        },
        DocumentVersion::new(1)?,
    )?;
    Ok((
        book,
        MergeRange::new(rows[0], rows[1], columns[0], columns[1]),
    ))
}

fn merge(
    sheet: SheetId,
    range: MergeRange,
    merged: bool,
    previous: bool,
    inverse: bool,
) -> Result<Operation, DomainError> {
    let (undoes, undoes_version) = if inverse {
        (
            Some(OperationId::from_bytes([11; 16])?),
            Some(DocumentVersion::new(2)?),
        )
    } else {
        (None, None)
    };
    Ok(Operation::MergeWrite {
        sheet,
        change: MergeChange {
            range,
            merged,
            previous,
        },
        undoes,
        undoes_version,
    })
}

#[test]
fn merge_events_roundtrip_geometry_preimage_and_undo_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let (book, range) = fixture()?;
    let sheet = book.sheets()[0].id();
    let authored = merge(sheet, range, true, false, false)?;
    let next = book.candidate(&authored, DocumentVersion::new(2)?)?;
    let encoded = encode(&event(authored, 2)?)?;
    let raw = v1::EventEnvelope::decode(encoded.as_slice())?;
    assert_eq!(raw.schema_version, 34);
    assert_eq!(
        decode(&encoded)?.operation,
        merge(sheet, range, true, false, false)?
    );

    let inverse = merge(sheet, range, false, true, true)?;
    let final_book = next.candidate(&inverse, DocumentVersion::new(3)?)?;
    let encoded_inverse = encode(&event(inverse.clone(), 3)?)?;
    let raw_inverse = v1::EventEnvelope::decode(encoded_inverse.as_slice())?;
    assert_eq!(raw_inverse.schema_version, 35);
    assert_eq!(decode(&encoded_inverse)?.operation, inverse);
    assert!(final_book.sheets()[0].merges().is_empty());
    Ok(())
}

#[test]
fn snapshot_schema_six_preserves_merge_state() -> Result<(), Box<dyn std::error::Error>> {
    let (book, range) = fixture()?;
    let merged = book.candidate(
        &merge(book.sheets()[0].id(), range, true, false, false)?,
        DocumentVersion::new(2)?,
    )?;
    let encoded = encode_snapshot(&merged, "engine-v1", "profile-v1")?;
    let raw = v1::SnapshotEnvelope::decode(encoded.as_slice())?;
    assert_eq!(raw.snapshot_schema_version, 7);
    assert_eq!(raw.sheets[0].merges.len(), 1);
    let restored = crate::decode_snapshot(
        &encoded,
        merged.document_id(),
        merged.version(),
        merged.last_axis_change_version(),
        merged.hash(),
        "engine-v1",
        "profile-v1",
    )?;
    assert_eq!(restored.hash(), merged.hash());
    assert_eq!(
        restored.sheets()[0]
            .merges()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![range]
    );
    Ok(())
}
