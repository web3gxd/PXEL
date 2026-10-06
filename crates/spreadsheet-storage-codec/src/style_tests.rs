use super::*;
use spreadsheet_domain::{style::*, style_write::prepare_style_write};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
fn fixture() -> TestResult<(Workbook, CommittedEvent)> {
    let sheet = SheetSeed {
        id: SheetId::from_bytes([4; 16])?,
        name: "Styles".into(),
        rows: vec![RowId::from_bytes([5; 16])?, RowId::from_bytes([6; 16])?],
        columns: vec![
            ColumnId::from_bytes([7; 16])?,
            ColumnId::from_bytes([8; 16])?,
        ],
    };
    let address = CellAddress {
        sheet: sheet.id,
        row: sheet.rows[0],
        column: sheet.columns[0],
    };
    let mut book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: sheet.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let initial = prepare_style_write(
        &book,
        &[address],
        &[StyleChange::Fill(Change::Set(Rgb::new([1, 2, 3])))],
        StyleWriteKind::Cell,
    )?;
    book = book.candidate(&initial, DocumentVersion::new(2)?)?;
    let addresses = sheet
        .rows
        .iter()
        .flat_map(|row| {
            sheet.columns.iter().map(move |column| CellAddress {
                sheet: sheet.id,
                row: *row,
                column: *column,
            })
        })
        .collect::<Vec<_>>();
    let patch = vec![
        StyleChange::FontBold(Change::Set(true)),
        StyleChange::Fill(Change::Set(Rgb::new([10, 20, 30]))),
        StyleChange::BorderTop(Change::Set(Border::new(
            BorderLine::Double,
            Rgb::new([40, 50, 60]),
        ))),
        StyleChange::NumberFormat(Change::Set(NumberFormat::Decimal2)),
    ];
    let operation = prepare_style_write(&book, &addresses, &patch, StyleWriteKind::Range)?;
    let candidate = book.candidate(&operation, DocumentVersion::new(3)?)?;
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([2; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
        version: candidate.version(),
        operation,
        state_hash: candidate.hash(),
    };
    Ok((book, event))
}
fn envelope(event: &CommittedEvent) -> TestResult<v1::EventEnvelope> {
    Ok(v1::EventEnvelope::decode(encode(event)?.as_slice())?)
}
fn raw_write(envelope: &mut v1::EventEnvelope) -> TestResult<&mut v1::StyleWrite> {
    match envelope
        .operation
        .as_mut()
        .and_then(|operation| operation.kind.as_mut())
    {
        Some(v1::operation::Kind::StyleWrite(write)) => Ok(write),
        _ => Err("style fixture".into()),
    }
}
#[test]
fn style_original_compensation_roundtrip_and_semantic_preimages() -> TestResult {
    let (book, event) = fixture()?;
    assert_eq!(schema_version(&event.operation), 23);
    let encoded = encode(&event)?;
    let decoded = decode(&encoded)?;
    assert_eq!(decoded, event);
    assert_eq!(event_hash(&decoded)?, event_hash(&event)?);
    let Operation::StyleWrite { kind, writes } = &event.operation else {
        return Err("style fixture".into());
    };
    assert_ne!(writes[0].previous, writes[1].previous);
    let inverse = Operation::StyleCompensation {
        kind: *kind,
        writes: writes
            .iter()
            .map(|write| GuardedStyleWrite {
                address: write.address,
                previous: write.changes.clone(),
                changes: write.previous.clone(),
            })
            .collect(),
        undoes: event.operation_id,
        undoes_version: event.version,
    };
    let after = book.candidate(&event.operation, event.version)?;
    let restored = after.candidate(&inverse, DocumentVersion::new(4)?)?;
    assert_eq!(restored.sheets()[0].styles(), book.sheets()[0].styles());
    let inverse_event = CommittedEvent {
        operation_id: OperationId::from_bytes([9; 16])?,
        version: restored.version(),
        operation: inverse,
        state_hash: restored.hash(),
        ..event.clone()
    };
    assert_eq!(schema_version(&inverse_event.operation), 24);
    assert_eq!(decode(&encode(&inverse_event)?)?, inverse_event);
    assert_ne!(event_hash(&event)?, event_hash(&inverse_event)?);
    let mut changed = event.clone();
    if let Operation::StyleWrite { writes, .. } = &mut changed.operation {
        writes[0].previous[0] = StyleChange::FontBold(Change::Set(true));
    }
    assert_ne!(event_hash(&changed)?, event_hash(&event)?);
    let mut system = event;
    system.principal = EventPrincipal::System;
    assert!(matches!(encode(&system), Err(CodecError::Invalid)));
    assert!(matches!(event_hash(&system), Err(CodecError::Invalid)));
    Ok(())
}
#[test]
fn style_malformed_boundaries_and_downgrade_are_rejected() -> TestResult {
    let (_, event) = fixture()?;
    let base = envelope(&event)?;
    let mut invalid = Vec::new();
    let mut e = base.clone();
    raw_write(&mut e)?.kind = 0;
    invalid.push(e);
    let mut e = base.clone();
    raw_write(&mut e)?.kind = 1;
    invalid.push(e);
    let mut e = base.clone();
    raw_write(&mut e)?.writes.clear();
    invalid.push(e);
    let mut e = base.clone();
    let w = raw_write(&mut e)?;
    w.writes = vec![w.writes[0].clone(); 1001];
    invalid.push(e);
    let mut e = base.clone();
    let w = raw_write(&mut e)?;
    w.writes[1] = w.writes[0].clone();
    invalid.push(e);
    let mut e = base.clone();
    raw_write(&mut e)?.writes.reverse();
    invalid.push(e);
    let mut e = base.clone();
    raw_write(&mut e)?.writes[0].address = None;
    invalid.push(e);
    let mut e = base.clone();
    raw_write(&mut e)?.writes[0].changes.clear();
    invalid.push(e);
    let mut e = base.clone();
    raw_write(&mut e)?.writes[0].previous.pop();
    invalid.push(e);
    let mut e = base.clone();
    let w = raw_write(&mut e)?;
    w.writes[0].changes[1] = w.writes[0].changes[0].clone();
    invalid.push(e);
    let mut e = base.clone();
    raw_write(&mut e)?.writes[0].changes.reverse();
    invalid.push(e);
    for (property, kind) in [
        (17, Some(v1::style_change::Kind::Reset(true))),
        (3, None),
        (3, Some(v1::style_change::Kind::Reset(false))),
        (3, Some(v1::style_change::Kind::Boolean(false))),
        (3, Some(v1::style_change::Kind::FontSize(1200))),
        (8, Some(v1::style_change::Kind::Rgb(0x100_0000))),
        (16, Some(v1::style_change::Kind::Enumeration(256))),
        (
            9,
            Some(v1::style_change::Kind::Border(v1::StyleBorder {
                line: 0,
                rgb: 0,
            })),
        ),
    ] {
        let mut e = base.clone();
        raw_write(&mut e)?.writes[0].changes[0] = v1::StyleChange { property, kind };
        invalid.push(e);
    }
    for e in invalid {
        assert!(decode(&e.encode_to_vec()).is_err());
    }
    let mut old = base.clone();
    old.schema_version = 22;
    assert!(matches!(
        decode(&old.encode_to_vec()),
        Err(CodecError::UnsupportedSchema)
    ));
    let mut unknown = base.clone();
    unknown.schema_version = 25;
    assert!(matches!(
        decode(&unknown.encode_to_vec()),
        Err(CodecError::UnsupportedSchema)
    ));
    let mut bytes = base.encode_to_vec();
    bytes.extend_from_slice(&[0xa0, 0x06, 1]);
    assert!(matches!(decode(&bytes), Err(CodecError::Invalid)));
    // Reordering known envelope fields is refused by the new writer-normalized profile.
    let bytes = base.encode_to_vec();
    let mut reordered = bytes[2..].to_vec();
    reordered.extend_from_slice(&bytes[..2]);
    assert!(matches!(decode(&reordered), Err(CodecError::Invalid)));
    Ok(())
}
#[test]
fn style_compensation_requires_past_target_and_exact_mask() -> TestResult {
    let (_, mut event) = fixture()?;
    let Operation::StyleWrite { kind, writes } = event.operation else {
        return Err("fixture".into());
    };
    event.operation = Operation::StyleCompensation {
        kind,
        writes,
        undoes: OperationId::from_bytes([10; 16])?,
        undoes_version: DocumentVersion::new(2)?,
    };
    assert!(decode(&encode(&event)?).is_ok());
    for target in [
        DocumentVersion::ZERO,
        event.version,
        DocumentVersion::new(4)?,
    ] {
        let mut malformed = event.clone();
        if let Operation::StyleCompensation { undoes_version, .. } = &mut malformed.operation {
            *undoes_version = target;
        }
        assert!(encode(&malformed).is_err());
        assert!(event_hash(&malformed).is_err());
    }
    let mut malformed = envelope(&event)?;
    if let Some(v1::operation::Kind::StyleCompensation(comp)) =
        malformed.operation.as_mut().and_then(|op| op.kind.as_mut())
    {
        comp.undoes_operation_id = vec![0; 16];
    }
    assert!(decode(&malformed.encode_to_vec()).is_err());
    Ok(())
}
#[test]
fn style_command_fingerprints_normalize_intent_and_preserve_mask() -> TestResult {
    let (_, event) = fixture()?;
    let Operation::StyleWrite { writes, .. } = &event.operation else {
        return Err("fixture".into());
    };
    let command = Command {
        document_id: event.document_id,
        operation_id: event.operation_id,
        actor_id: ActorId::from_bytes([3; 16])?,
        base_version: DocumentVersion::new(2)?,
        client_sequence: 1,
        operation: CommandPayload::SetRangeStyle {
            addresses: writes.iter().map(|w| w.address).collect(),
            changes: vec![
                StyleChange::FontBold(Change::Set(false)),
                StyleChange::Fill(Change::Set(Rgb::new([1, 2, 3]))),
            ],
        },
    };
    let mut reordered = command.clone();
    if let CommandPayload::SetRangeStyle { addresses, changes } = &mut reordered.operation {
        addresses.reverse();
        changes.reverse();
        changes[1] = StyleChange::FontBold(Change::Reset);
    }
    assert_eq!(
        command_fingerprint(&command)?,
        command_fingerprint(&reordered)?
    );
    let mut mask = command.clone();
    if let CommandPayload::SetRangeStyle { changes, .. } = &mut mask.operation {
        changes.remove(0);
    }
    assert_ne!(command_fingerprint(&command)?, command_fingerprint(&mask)?);
    let mut cell = command.clone();
    cell.operation = CommandPayload::SetCellStyle {
        address: writes[0].address,
        changes: vec![StyleChange::FontBold(Change::Reset)],
    };
    let mut range = cell.clone();
    range.operation = CommandPayload::SetRangeStyle {
        addresses: vec![writes[0].address],
        changes: vec![StyleChange::FontBold(Change::Reset)],
    };
    assert_ne!(command_fingerprint(&cell)?, command_fingerprint(&range)?);
    Ok(())
}
fn restored(bytes: &[u8], book: &Workbook) -> Result<Workbook, CodecError> {
    decode_snapshot(
        bytes,
        book.document_id(),
        book.version(),
        book.last_axis_change_version(),
        book.hash(),
        "engine-v1",
        "pxel-v1",
    )
}
#[test]
fn snapshot_four_retains_styles_and_refuses_old_profiles_and_default_entries() -> TestResult {
    let (book, event) = fixture()?;
    let book = book.candidate(&event.operation, event.version)?;
    let bytes = encode_snapshot(&book, "engine-v1", "pxel-v1")?;
    let raw = v1::SnapshotEnvelope::decode(bytes.as_slice())?;
    assert_eq!(raw.snapshot_schema_version, crate::SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(raw.sheets[0].styles.len(), 4);
    let decoded = restored(&bytes, &book)?;
    assert_eq!(decoded.hash(), book.hash());
    assert_eq!(decoded.sheets()[0].styles(), book.sheets()[0].styles());
    for schema in [1, 2, 3] {
        let mut old = raw.clone();
        old.snapshot_schema_version = schema;
        assert!(restored(&old.encode_to_vec(), &book).is_err());
    }
    let mut duplicate = raw.clone();
    let duplicated_style = duplicate.sheets[0].styles[0].clone();
    duplicate.sheets[0].styles.push(duplicated_style);
    assert!(restored(&duplicate.encode_to_vec(), &book).is_err());
    let mut default = raw.clone();
    default.sheets[0].styles[0]
        .style
        .as_mut()
        .ok_or("style")?
        .changes
        .clear();
    assert!(restored(&default.encode_to_vec(), &book).is_err());
    let mut unknown = raw.clone();
    unknown.sheets[0].styles[0]
        .style
        .as_mut()
        .ok_or("style")?
        .profile_version = 3;
    assert!(matches!(
        restored(&unknown.encode_to_vec(), &book),
        Err(CodecError::UnsupportedSchema)
    ));
    let mut extra = bytes;
    extra.extend_from_slice(&[0xa0, 0x06, 1]);
    assert!(restored(&extra, &book).is_err());
    Ok(())
}

#[test]
fn style_all_sixteen_leaves_roundtrip_in_event_and_snapshot() -> TestResult {
    let (book, _) = fixture()?;
    let sheet = &book.sheets()[0];
    let address = CellAddress {
        sheet: sheet.id(),
        row: sheet.rows()[0],
        column: sheet.columns()[0],
    };
    let border = Border::new(BorderLine::Dashed, Rgb::new([0, 128, 255]));
    let patch = vec![
        StyleChange::FontFamily(Change::Set(FontFamily::new("Noto Sans".into())?)),
        StyleChange::FontSize(Change::Set(FontSize::new(20000)?)),
        StyleChange::FontBold(Change::Set(true)),
        StyleChange::FontItalic(Change::Set(true)),
        StyleChange::FontUnderline(Change::Set(Underline::Double)),
        StyleChange::FontStrike(Change::Set(true)),
        StyleChange::FontColor(Change::Set(Rgb::new([255, 0, 128]))),
        StyleChange::Fill(Change::Set(Rgb::new([1, 2, 3]))),
        StyleChange::BorderTop(Change::Set(border)),
        StyleChange::BorderRight(Change::Set(border)),
        StyleChange::BorderBottom(Change::Set(border)),
        StyleChange::BorderLeft(Change::Set(border)),
        StyleChange::Horizontal(Change::Set(HorizontalAlignment::Right)),
        StyleChange::Vertical(Change::Set(VerticalAlignment::Top)),
        StyleChange::TextOverflow(Change::Set(TextOverflow::Wrap)),
        StyleChange::NumberFormat(Change::Set(NumberFormat::DateIso)),
    ];
    let operation = prepare_style_write(&book, &[address], &patch, StyleWriteKind::Cell)?;
    let candidate = book.candidate(&operation, DocumentVersion::new(3)?)?;
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([12; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
        version: candidate.version(),
        operation,
        state_hash: candidate.hash(),
    };
    assert_eq!(decode(&encode(&event)?)?, event);
    let bytes = encode_snapshot(&candidate, "engine-v1", "pxel-v1")?;
    let decoded = restored(&bytes, &candidate)?;
    assert_eq!(decoded.sheets()[0].styles(), candidate.sheets()[0].styles());
    assert_eq!(decoded.hash(), candidate.hash());
    Ok(())
}

#[test]
fn overflow_profile_event_and_snapshot_roundtrip_while_legacy_wrap_stays_readable() -> TestResult {
    let (book, _) = fixture()?;
    let sheet = &book.sheets()[0];
    let address = CellAddress {
        sheet: sheet.id(),
        row: sheet.rows()[0],
        column: sheet.columns()[0],
    };
    let operation = prepare_style_write(
        &book,
        &[address],
        &[StyleChange::TextOverflow(Change::Set(
            TextOverflow::Overflow,
        ))],
        StyleWriteKind::Cell,
    )?;
    let candidate = book.candidate(&operation, DocumentVersion::new(3)?)?;
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([18; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
        version: candidate.version(),
        operation,
        state_hash: candidate.hash(),
    };
    let mut raw = envelope(&event)?;
    let stored = raw_write(&mut raw)?;
    assert_eq!(stored.profile_version, 2);
    assert!(matches!(
        stored.writes[0].changes[0].kind,
        Some(v1::style_change::Kind::Enumeration(2))
    ));
    assert_eq!(decode(&raw.encode_to_vec())?, event);

    let snapshot = encode_snapshot(&candidate, "engine-v1", "pxel-v1")?;
    let snapshot = v1::SnapshotEnvelope::decode(snapshot.as_slice())?;
    assert!(snapshot.sheets[0].styles.iter().any(|entry| {
        entry
            .style
            .as_ref()
            .is_some_and(|style| style.profile_version == 2)
    }));
    let snapshot_bytes = snapshot.encode_to_vec();
    let decoded = restored(&snapshot_bytes, &candidate)?;
    assert_eq!(decoded.hash(), candidate.hash());
    assert_eq!(decoded.sheets()[0].styles(), candidate.sheets()[0].styles());

    let wrap = prepare_style_write(
        &book,
        &[address],
        &[StyleChange::TextOverflow(Change::Set(TextOverflow::Wrap))],
        StyleWriteKind::Cell,
    )?;
    let wrap_book = book.candidate(&wrap, DocumentVersion::new(3)?)?;
    let wrap_event = CommittedEvent {
        operation_id: OperationId::from_bytes([19; 16])?,
        operation: wrap,
        state_hash: wrap_book.hash(),
        ..event
    };
    let mut legacy = envelope(&wrap_event)?;
    let stored = raw_write(&mut legacy)?;
    assert_eq!(stored.profile_version, 1);
    assert!(matches!(
        stored.writes[0].changes[0].kind,
        Some(v1::style_change::Kind::Boolean(true))
    ));
    stored.profile_version = 0; // Old event payloads omitted this field.
    assert_eq!(decode(&legacy.encode_to_vec())?, wrap_event);
    Ok(())
}

#[test]
fn snapshot_four_roundtrips_multisheet_content_above_legacy_aggregate_cap() -> TestResult {
    fn stable(prefix: u8, index: u32) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[0] = prefix;
        bytes[12..].copy_from_slice(&index.to_be_bytes());
        bytes
    }
    let mut seeds = Vec::new();
    let mut cells = Vec::new();
    let mut styled_blank = None;
    for prefix in [10u8, 20] {
        let sheet = SheetId::from_bytes(stable(prefix, 0))?;
        let column = ColumnId::from_bytes(stable(prefix + 1, 0))?;
        let rows = (0..50_002)
            .map(|index| RowId::from_bytes(stable(prefix + 2, index)))
            .collect::<Result<Vec<_>, _>>()?;
        cells.extend(rows[..50_001].iter().map(|row| {
            (
                CellAddress {
                    sheet,
                    row: *row,
                    column,
                },
                CellValue::Number(1.).into(),
            )
        }));
        styled_blank = Some(CellAddress {
            sheet,
            row: rows[50_001],
            column,
        });
        seeds.push(SheetSeed {
            id: sheet,
            name: format!("Sheet{prefix}"),
            rows,
            columns: vec![column],
        });
    }
    let book = Workbook::hydrate_sheets(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        seeds,
        cells,
    )?;
    let address = styled_blank.ok_or("styled blank")?;
    let operation = prepare_style_write(
        &book,
        &[address],
        &[StyleChange::FontBold(Change::Set(true))],
        StyleWriteKind::Cell,
    )?;
    let book = book.candidate(&operation, DocumentVersion::new(2)?)?;
    let bytes = encode_snapshot(&book, "engine-v1", "pxel-v1")?;
    assert!(bytes.len() < MAX_SNAPSHOT_BYTES);
    let decoded = restored(&bytes, &book)?;
    assert_eq!(decoded.hash(), book.hash());
    assert_eq!(
        decoded
            .sheets()
            .iter()
            .map(|sheet| sheet.cells().len())
            .sum::<usize>(),
        100_002
    );
    assert_eq!(decoded.sheets()[1].styles(), book.sheets()[1].styles());
    let mut legacy = v1::SnapshotEnvelope::decode(bytes.as_slice())?;
    legacy.snapshot_schema_version = 3;
    for sheet in &mut legacy.sheets {
        sheet.settings = None;
    }
    for sheet in &mut legacy.sheets {
        sheet.styles.clear();
    }
    assert!(restored(&legacy.encode_to_vec(), &book).is_err());
    Ok(())
}
