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
    assert_eq!(raw.snapshot_schema_version, crate::SNAPSHOT_SCHEMA_VERSION);
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

#[test]
fn destructive_merge_roundtrips_preimages_and_compensation()
-> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::style::{Change, StyleChange};
    use spreadsheet_domain::workbook::{
        CellAddress, CellContent, CellValue, GuardedContentWrite, GuardedStyleWrite,
    };
    let (book, range) = fixture()?;
    let sheet = book.sheets()[0].id();
    let address = CellAddress {
        sheet,
        row: range.last_row(),
        column: range.last_column(),
    };
    for inverse in [false, true] {
        let original = Some(CellContent::Value(CellValue::Text("covered".into())));
        let (content, previous) = if inverse {
            (original, None)
        } else {
            (None, original)
        };
        let (changes, style_previous) = if inverse {
            (
                vec![StyleChange::FontBold(Change::Set(true))],
                vec![StyleChange::FontBold(Change::Reset)],
            )
        } else {
            (
                vec![StyleChange::FontBold(Change::Reset)],
                vec![StyleChange::FontBold(Change::Set(true))],
            )
        };
        let operation = Operation::MergeCellsWrite {
            sheet,
            change: MergeChange {
                range,
                merged: !inverse,
                previous: inverse,
            },
            content_writes: vec![GuardedContentWrite {
                address,
                content,
                previous,
            }],
            style_writes: vec![GuardedStyleWrite {
                address,
                changes,
                previous: style_previous,
            }],
            undoes: if inverse {
                Some(OperationId::from_bytes([11; 16])?)
            } else {
                None
            },
            undoes_version: if inverse {
                Some(DocumentVersion::new(2)?)
            } else {
                None
            },
        };
        let version = if inverse { 3 } else { 2 };
        let hash = crate::event_hash(&event(operation.clone(), version)?)?;
        let mut changed = operation.clone();
        if let Operation::MergeCellsWrite { content_writes, .. } = &mut changed {
            let target = if inverse {
                &mut content_writes[0].content
            } else {
                &mut content_writes[0].previous
            };
            *target = Some(CellContent::Value(CellValue::Text(
                "changed preimage".into(),
            )));
        }
        assert_ne!(hash, crate::event_hash(&event(changed, version)?)?);
        let mut oversized = operation.clone();
        if let Operation::MergeCellsWrite { content_writes, .. } = &mut oversized {
            *content_writes = vec![content_writes[0].clone(); 1_001];
        }
        assert!(encode(&event(oversized, version)?).is_err());
        let encoded = encode(&event(operation.clone(), version)?)?;
        let dto = v1::EventEnvelope::decode(encoded.as_slice())?;
        assert_eq!(dto.schema_version, if inverse { 44 } else { 43 });
        assert_eq!(decode(&encoded)?.operation, operation);
        let mut downgraded = dto.clone();
        downgraded.schema_version = 34;
        assert!(decode(&downgraded.encode_to_vec()).is_err());
        let mut malformed = dto;
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::MergeCellsWrite(write)),
        }) = &mut malformed.operation
        {
            write.content_writes[0].address = None;
        }
        assert!(decode(&malformed.encode_to_vec()).is_err());
    }
    Ok(())
}
