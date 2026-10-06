use spreadsheet_domain::{style::*, style_write::*, workbook::*};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn frozen_range_inverse_survives_interior_axis_insertions() -> TestResult {
    for insert_rows in [true, false] {
        let (book, a) = fixture()?;
        let sheet = &book.sheets()[0];
        let b = if insert_rows {
            CellAddress {
                row: sheet.rows()[1],
                ..a
            }
        } else {
            CellAddress {
                column: sheet.columns()[1],
                ..a
            }
        };
        let operation = prepare_style_write(
            &book,
            &[a, b],
            &[StyleChange::FontBold(Change::Set(true))],
            StyleWriteKind::Range,
        )?;
        let styled = book.candidate(&operation, DocumentVersion::new(2)?)?;
        let insertion = if insert_rows {
            Operation::InsertRowsRebased {
                sheet: a.sheet,
                before: b.row,
                rows: vec![RowId::from_bytes([17; 16])?],
            }
        } else {
            Operation::InsertColumnsRebased {
                sheet: a.sheet,
                before: b.column,
                columns: vec![ColumnId::from_bytes([18; 16])?],
            }
        };
        let axis = spreadsheet_domain::axis_edit::prepare_axis_edit(
            &styled,
            &insertion,
            DocumentVersion::new(3)?,
        )?;
        let inserted = styled.candidate(&axis, DocumentVersion::new(3)?)?;
        // New authored commands still require a rectangle at their own head.
        assert!(
            prepare_style_write(
                &inserted,
                &[a, b],
                &[StyleChange::FontBold(Change::Reset)],
                StyleWriteKind::Range
            )
            .is_err()
        );
        let Operation::StyleWrite { kind, writes } = operation else {
            return Err("style write".into());
        };
        let inverse = Operation::StyleCompensation {
            kind,
            writes: writes
                .iter()
                .map(|w| GuardedStyleWrite {
                    address: w.address,
                    previous: w.changes.clone(),
                    changes: w.previous.clone(),
                })
                .collect(),
            undoes: OperationId::from_bytes([19; 16])?,
            undoes_version: DocumentVersion::new(2)?,
        };
        let undone = inserted.candidate(&inverse, DocumentVersion::new(4)?)?;
        assert!(undone.sheets()[0].styles().is_empty());
        let redo = Operation::StyleCompensation {
            kind,
            writes,
            undoes: OperationId::from_bytes([20; 16])?,
            undoes_version: DocumentVersion::new(4)?,
        };
        let redone = undone.candidate(&redo, DocumentVersion::new(5)?)?;
        assert_eq!(redone.sheets()[0].styles(), styled.sheets()[0].styles());
    }
    Ok(())
}

#[test]
fn inverse_restores_only_written_properties_and_revival_checks_style_intents() -> TestResult {
    let (book, a) = fixture()?;
    let op = prepare_style_write(
        &book,
        &[a],
        &[StyleChange::FontBold(Change::Set(true))],
        StyleWriteKind::Cell,
    )?;
    let Operation::StyleWrite { kind, writes } = &op else {
        return Err("unexpected operation".into());
    };
    let styled = book.candidate(&op, DocumentVersion::new(2)?)?;
    let italic = prepare_style_write(
        &styled,
        &[a],
        &[StyleChange::FontItalic(Change::Set(true))],
        StyleWriteKind::Cell,
    )?;
    let styled = styled.candidate(&italic, DocumentVersion::new(3)?)?;
    let inverse = Operation::StyleCompensation {
        kind: *kind,
        writes: writes
            .iter()
            .map(|w| GuardedStyleWrite {
                address: w.address,
                previous: w.changes.clone(),
                changes: w.previous.clone(),
            })
            .collect(),
        undoes: OperationId::from_bytes([8; 16])?,
        undoes_version: DocumentVersion::new(2)?,
    };
    let undone = styled.candidate(&inverse, DocumentVersion::new(4)?)?;
    assert!(!undone.sheets()[0].styles()[&a].font_bold());
    assert!(undone.sheets()[0].styles()[&a].font_italic());
    let delete = spreadsheet_domain::axis_edit::prepare_axis_edit(
        &book,
        &Operation::DeleteRows {
            sheet: a.sheet,
            rows: vec![a.row],
        },
        DocumentVersion::new(2)?,
    )?;
    let deleted = book.candidate(&delete, DocumentVersion::new(2)?)?;
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([9; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([10; 16])?),
        version: deleted.version(),
        operation: delete,
        state_hash: deleted.hash(),
    };
    let inverse = spreadsheet_domain::axis_edit::prepare_axis_inverse(
        &deleted,
        &event,
        DocumentVersion::new(3)?,
    )?;
    let restored = deleted.candidate(&inverse, DocumentVersion::new(3)?)?;
    for operation in [
        CommandPayload::SetCellStyle {
            address: a,
            changes: vec![StyleChange::FontBold(Change::Reset)],
        },
        CommandPayload::SetRangeStyle {
            addresses: vec![a],
            changes: vec![StyleChange::FontBold(Change::Reset)],
        },
    ] {
        let command = Command {
            document_id: book.document_id(),
            operation_id: OperationId::from_bytes([11; 16])?,
            actor_id: ActorId::from_bytes([10; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation,
        };
        assert_eq!(
            spreadsheet_domain::lifecycle::check_command_activation(&restored, &command),
            Err(DomainError::VersionConflict)
        );
    }
    Ok(())
}
fn fixture() -> Result<(Workbook, CellAddress), DomainError> {
    let address = CellAddress {
        sheet: SheetId::from_bytes([2; 16])?,
        row: RowId::from_bytes([3; 16])?,
        column: ColumnId::from_bytes([4; 16])?,
    };
    Ok((
        Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: address.sheet,
                    name: "Sheet1".into(),
                    rows: vec![address.row, RowId::from_bytes([5; 16])?],
                    columns: vec![address.column, ColumnId::from_bytes([6; 16])?],
                },
            },
            DocumentVersion::new(1)?,
        )?,
        address,
    ))
}

#[test]
fn cell_batch_applies_values_and_alignment_and_undoes_together() -> TestResult {
    let (book, first) = fixture()?;
    let sheet = &book.sheets()[0];
    let second = CellAddress {
        row: sheet.rows()[1],
        ..first
    };
    let content_writes = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, address)| GuardedContentWrite {
            address,
            content: Some(CellContent::Value(CellValue::Text(format!("row {index}")))),
            previous: None,
        })
        .collect();
    let style_operation = prepare_style_write(
        &book,
        &[first, second],
        &[StyleChange::Horizontal(Change::Set(
            HorizontalAlignment::Center,
        ))],
        StyleWriteKind::Range,
    )?;
    let Operation::StyleWrite {
        writes: style_writes,
        ..
    } = style_operation
    else {
        return Err("expected style preimages".into());
    };
    let authored = Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::SetValues,
        content_writes,
        style_writes,
        undoes: None,
        undoes_version: None,
    };
    let applied = book.candidate(&authored, DocumentVersion::new(2)?)?;
    assert_eq!(
        applied.cell_content(first)?.map(CellContent::text_bytes),
        Some(5)
    );
    assert_eq!(
        applied.sheets()[0].styles()[&first].horizontal(),
        HorizontalAlignment::Center
    );

    let Operation::CellBatchWrite {
        content_writes,
        style_writes,
        ..
    } = &authored
    else {
        return Err("expected cell batch".into());
    };
    let inverse = Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::SetValues,
        content_writes: content_writes
            .iter()
            .map(|write| GuardedContentWrite {
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
        undoes: Some(OperationId::from_bytes([9; 16])?),
        undoes_version: Some(DocumentVersion::new(2)?),
    };
    let undone = applied.candidate(&inverse, DocumentVersion::new(3)?)?;
    assert!(undone.cell_content(first)?.is_none());
    assert!(undone.sheets()[0].styles().is_empty());
    Ok(())
}

#[test]
fn independent_properties_commute_but_stale_same_leaf_is_rejected() -> TestResult {
    let (book, a) = fixture()?;
    let bold = prepare_style_write(
        &book,
        &[a],
        &[StyleChange::FontBold(Change::Set(true))],
        StyleWriteKind::Cell,
    )?;
    let italic = prepare_style_write(
        &book,
        &[a],
        &[StyleChange::FontItalic(Change::Set(true))],
        StyleWriteKind::Cell,
    )?;
    let left = book
        .candidate(&bold, DocumentVersion::new(2)?)?
        .candidate(&italic, DocumentVersion::new(3)?)?;
    let right = book
        .candidate(&italic, DocumentVersion::new(2)?)?
        .candidate(&bold, DocumentVersion::new(3)?)?;
    assert_eq!(left.hash(), right.hash());
    assert!(matches!(
        left.candidate(&bold, DocumentVersion::new(4)?),
        Err(DomainError::PreimageConflict)
    ));
    assert!(left.sheets()[0].styles()[&a].font_bold());
    Ok(())
}

#[test]
fn defaults_keep_explicit_footprint_and_invalid_guards_are_atomic() -> TestResult {
    let (book, a) = fixture()?;
    let op = prepare_style_write(
        &book,
        &[a],
        &[StyleChange::FontBold(Change::Set(false))],
        StyleWriteKind::Cell,
    )?;
    let Operation::StyleWrite { kind, writes } = op else {
        return Err("unexpected operation".into());
    };
    assert_eq!(
        writes[0].changes,
        vec![StyleChange::FontBold(Change::Reset)]
    );
    assert_eq!(writes[0].previous, writes[0].changes);
    let candidate = book.candidate(
        &Operation::StyleWrite {
            kind,
            writes: writes.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let content_only = book.candidate(
        &Operation::ClearCell { address: a },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(candidate.hash(), content_only.hash());
    let mut forged = writes.clone();
    forged[0].changes.push(StyleChange::FontBold(Change::Reset));
    assert!(matches!(
        book.candidate(
            &Operation::StyleWrite {
                kind,
                writes: forged
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::InvalidStyle)
    ));
    assert!(
        prepare_style_write(
            &book,
            &[a, a],
            &[StyleChange::FontBold(Change::Reset)],
            StyleWriteKind::Range
        )
        .is_err()
    );
    let mut forged = writes;
    forged[0].previous = vec![StyleChange::FontItalic(Change::Reset)];
    assert!(matches!(
        book.candidate(
            &Operation::StyleWrite {
                kind,
                writes: forged
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::InvalidStyle)
    ));
    assert!(book.sheets()[0].styles().is_empty());
    Ok(())
}

#[test]
fn styled_axis_deletion_undo_and_redo_preserve_exact_style_preimages() -> TestResult {
    let (book, a) = fixture()?;
    let op = prepare_style_write(
        &book,
        &[a],
        &[StyleChange::FontBold(Change::Set(true))],
        StyleWriteKind::Cell,
    )?;
    let book = book.candidate(&op, DocumentVersion::new(2)?)?;
    let delete = Operation::DeleteRows {
        sheet: a.sheet,
        rows: vec![a.row],
    };
    let delete =
        spreadsheet_domain::axis_edit::prepare_axis_edit(&book, &delete, DocumentVersion::new(3)?)?;
    let deleted = book.candidate(&delete, DocumentVersion::new(3)?)?;
    assert!(deleted.sheets()[0].styles().is_empty());
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([30; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([31; 16])?),
        version: deleted.version(),
        operation: delete,
        state_hash: deleted.hash(),
    };
    let inverse = spreadsheet_domain::axis_edit::prepare_axis_inverse(
        &deleted,
        &event,
        DocumentVersion::new(4)?,
    )?;
    let restored = deleted.candidate(&inverse, DocumentVersion::new(4)?)?;
    assert_eq!(restored.sheets()[0].styles(), book.sheets()[0].styles());
    let undo_event = CommittedEvent {
        document_id: restored.document_id(),
        operation_id: OperationId::from_bytes([32; 16])?,
        principal: event.principal,
        version: restored.version(),
        operation: inverse,
        state_hash: restored.hash(),
    };
    let redo = spreadsheet_domain::axis_edit::prepare_axis_inverse(
        &restored,
        &undo_event,
        DocumentVersion::new(5)?,
    )?;
    let redone = restored.candidate(&redo, DocumentVersion::new(5)?)?;
    assert!(redone.sheets()[0].styles().is_empty());
    let delete_column = spreadsheet_domain::axis_edit::prepare_axis_edit(
        &book,
        &Operation::DeleteColumns {
            sheet: a.sheet,
            columns: vec![a.column],
        },
        DocumentVersion::new(3)?,
    )?;
    let deleted_column = book.candidate(&delete_column, DocumentVersion::new(3)?)?;
    assert!(deleted_column.sheets()[0].styles().is_empty());
    let column_event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([33; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([31; 16])?),
        version: deleted_column.version(),
        operation: delete_column,
        state_hash: deleted_column.hash(),
    };
    let column_undo = spreadsheet_domain::axis_edit::prepare_axis_inverse(
        &deleted_column,
        &column_event,
        DocumentVersion::new(4)?,
    )?;
    let restored_column = deleted_column.candidate(&column_undo, DocumentVersion::new(4)?)?;
    assert_eq!(
        restored_column.sheets()[0].styles(),
        book.sheets()[0].styles()
    );
    let next = book.candidate(
        &Operation::DeleteRows {
            sheet: a.sheet,
            rows: vec![RowId::from_bytes([5; 16])?],
        },
        DocumentVersion::new(3)?,
    )?;
    assert!(next.sheets()[0].styles()[&a].font_bold());
    let mut image = book.snapshot_image()?;
    image.sheets[0].styles.push((a, CellStyle::default()));
    assert!(matches!(
        Workbook::restore_snapshot(image, book.hash()),
        Err(DomainError::InvalidStyle)
    ));
    Ok(())
}

#[test]
fn union_limit_counts_overlap_once_and_batch_uses_final_occupancy() -> TestResult {
    let (_, a) = fixture()?;
    let rows = (1u128..=50_001)
        .map(|n| RowId::from_bytes(n.to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let second = ColumnId::from_bytes([6; 16])?;
    let seed = SheetSeed {
        id: a.sheet,
        name: "Sheet1".into(),
        rows: rows.clone(),
        columns: vec![a.column, second],
    };
    let cells = rows.iter().take(50_000).flat_map(|row| {
        [a.column, second].map(|column| {
            (
                CellAddress {
                    sheet: a.sheet,
                    row: *row,
                    column,
                },
                CellContent::Value(CellValue::Number(1.0)),
            )
        })
    });
    let book = Workbook::hydrate(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(1)?,
        seed,
        cells,
    )?;
    let existing = CellAddress { row: rows[0], ..a };
    let empty = CellAddress {
        row: rows[50_000],
        ..a
    };
    let patch = [StyleChange::FontBold(Change::Set(true))];
    let styled = book.candidate(
        &prepare_style_write(&book, &[existing], &patch, StyleWriteKind::Cell)?,
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(styled.sheets()[0].cells().len(), MAX_CELLS);
    assert!(matches!(
        styled.candidate(
            &prepare_style_write(&styled, &[empty], &patch, StyleWriteKind::Cell)?,
            DocumentVersion::new(3)?
        ),
        Err(DomainError::LimitExceeded)
    ));
    let cleared = styled.candidate(
        &Operation::ClearCell { address: existing },
        DocumentVersion::new(3)?,
    )?;
    assert!(matches!(
        cleared.candidate(
            &Operation::SetCellValue {
                address: empty,
                value: CellValue::Number(2.0)
            },
            DocumentVersion::new(4)?
        ),
        Err(DomainError::LimitExceeded)
    ));
    let replacement = CellAddress { row: rows[1], ..a };
    let final_book = cleared.preview_candidate(
        &[
            Operation::SetCellValue {
                address: empty,
                value: CellValue::Number(2.0),
            },
            Operation::ClearCell {
                address: replacement,
            },
        ],
        DocumentVersion::new(4)?,
    )?;
    assert!(final_book.sheets()[0].cells().contains_key(&empty));
    let deletion = spreadsheet_domain::axis_edit::prepare_axis_edit(
        &book,
        &Operation::DeleteRows {
            sheet: a.sheet,
            rows: vec![rows[0]],
        },
        DocumentVersion::new(2)?,
    )?;
    let deleted = book.candidate(&deletion, DocumentVersion::new(2)?)?;
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([12; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([13; 16])?),
        version: deleted.version(),
        operation: deletion,
        state_hash: deleted.hash(),
    };
    let style = prepare_style_write(
        &deleted,
        &[
            empty,
            CellAddress {
                column: second,
                ..empty
            },
        ],
        &patch,
        StyleWriteKind::Range,
    )?;
    let deleted = deleted.candidate(&style, DocumentVersion::new(3)?)?;
    assert!(
        spreadsheet_domain::axis_edit::prepare_axis_inverse(
            &deleted,
            &event,
            DocumentVersion::new(4)?
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn styled_blank_survives_content_clear_and_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let address = CellAddress {
        sheet: SheetId::from_bytes([2; 16])?,
        row: RowId::from_bytes([3; 16])?,
        column: ColumnId::from_bytes([4; 16])?,
    };
    let book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: address.sheet,
                name: "Sheet1".into(),
                rows: vec![address.row],
                columns: vec![address.column],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    let operation = prepare_style_write(
        &book,
        &[address],
        &[StyleChange::FontBold(Change::Set(true))],
        StyleWriteKind::Cell,
    )?;
    let styled = book.candidate(&operation, DocumentVersion::new(2)?)?;
    let cleared = styled.candidate(&Operation::ClearCell { address }, DocumentVersion::new(3)?)?;
    assert!(cleared.sheets()[0].cells().is_empty());
    assert!(cleared.sheets()[0].styles()[&address].font_bold());
    let restored = Workbook::restore_snapshot(cleared.snapshot_image()?, cleared.hash())?;
    assert_eq!(restored.hash(), cleared.hash());
    Ok(())
}
