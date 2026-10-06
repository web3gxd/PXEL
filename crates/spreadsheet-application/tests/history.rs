use spreadsheet_application::{history::*, journal::JournalError};
use spreadsheet_domain::workbook::*;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[test]
fn compensation_cannot_claim_wrong_identity_for_a_visible_version() -> TestResult {
    for claimed in [
        HistoryTarget {
            committed_version: DocumentVersion::new(1)?,
            ..target(3)?
        },
        HistoryTarget {
            committed_version: DocumentVersion::new(1)?,
            ..target(99)?
        },
    ] {
        assert!(matches!(
            reconstruct(window(vec![
                entry(1, HistoryEntryKind::Original)?,
                entry(2, HistoryEntryKind::Compensation { target: claimed })?,
                entry(3, HistoryEntryKind::Original)?
            ])?),
            Err(JournalError::Corrupt)
        ));
    }
    Ok(())
}
fn target(version: u64) -> Result<HistoryTarget, DomainError> {
    Ok(HistoryTarget {
        operation_id: OperationId::from_bytes(u128::from(version).to_be_bytes())?,
        committed_version: DocumentVersion::new(version)?,
    })
}
fn entry(version: u64, kind: HistoryEntryKind) -> Result<HistoryEntry, DomainError> {
    Ok(HistoryEntry {
        target: target(version)?,
        kind,
    })
}
fn window(entries: Vec<HistoryEntry>) -> Result<HistoryWindow, DomainError> {
    Ok(HistoryWindow {
        document_id: DocumentId::from_bytes([1; 16])?,
        actor_id: ActorId::from_bytes([2; 16])?,
        through_version: entries
            .last()
            .map_or(DocumentVersion::ZERO, |e| e.target.committed_version),
        earliest_scanned_version: entries
            .first()
            .map_or(DocumentVersion::ZERO, |e| e.target.committed_version),
        entries,
        truncation: None,
    })
}
#[test]
fn nonlinear_undo_and_redo_preserve_independent_stacks() -> TestResult {
    let history = reconstruct(window(vec![
        entry(1, HistoryEntryKind::Original)?,
        entry(2, HistoryEntryKind::Original)?,
        entry(3, HistoryEntryKind::Compensation { target: target(1)? })?,
    ])?)?;
    assert_eq!(history.undo, vec![target(2)?]);
    assert_eq!(history.redo, vec![target(3)?]);
    let history = reconstruct(window(vec![
        entry(1, HistoryEntryKind::Original)?,
        entry(2, HistoryEntryKind::Original)?,
        entry(3, HistoryEntryKind::Compensation { target: target(1)? })?,
        entry(4, HistoryEntryKind::Compensation { target: target(2)? })?,
        entry(5, HistoryEntryKind::Compensation { target: target(3)? })?,
    ])?)?;
    assert_eq!(history.undo, vec![target(5)?]);
    assert_eq!(history.redo, vec![target(4)?]);
    Ok(())
}
#[test]
fn branching_unknown_inverse_and_barriers_are_explicit() -> TestResult {
    let history = reconstruct(window(vec![
        entry(1, HistoryEntryKind::Original)?,
        entry(2, HistoryEntryKind::Compensation { target: target(1)? })?,
        entry(3, HistoryEntryKind::Original)?,
        entry(4, HistoryEntryKind::Compensation { target: target(2)? })?,
    ])?)?;
    assert_eq!(history.undo, vec![target(3)?, target(4)?]);
    assert!(history.redo.is_empty());
    let history = reconstruct(window(vec![
        entry(1, HistoryEntryKind::Original)?,
        entry(2, HistoryEntryKind::Ignore)?,
        entry(3, HistoryEntryKind::Barrier)?,
        entry(4, HistoryEntryKind::Original)?,
    ])?)?;
    assert_eq!(history.undo, vec![target(4)?]);
    Ok(())
}
#[test]
fn caps_keep_latest_targets_and_inverse_outside_stack_is_new_original() -> TestResult {
    let mut entries = (1..=101)
        .map(|v| entry(v, HistoryEntryKind::Original))
        .collect::<Result<Vec<_>, _>>()?;
    let history = reconstruct(window(entries.clone())?)?;
    assert_eq!(history.undo.len(), 100);
    assert_eq!(history.undo[0], target(2)?);
    entries.push(entry(
        102,
        HistoryEntryKind::Compensation { target: target(1)? },
    )?);
    let history = reconstruct(window(entries)?)?;
    assert_eq!(history.undo.len(), 100);
    assert_eq!(history.undo[0], target(3)?);
    assert_eq!(history.undo.last(), Some(&target(102)?));
    Ok(())
}
#[test]
fn suffix_boundaries_truncation_and_empty_head_are_checked() -> TestResult {
    assert!(reconstruct(window(vec![])?)?.undo.is_empty());
    let entries = (1..=10001)
        .map(|v| entry(v, HistoryEntryKind::Ignore))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(reconstruct(window(entries.clone())?).is_ok());
    let mut too_many = entries;
    too_many.push(entry(10002, HistoryEntryKind::Ignore)?);
    assert!(matches!(
        reconstruct(window(too_many)?),
        Err(JournalError::Corrupt)
    ));
    let mut suffix = window(vec![entry(
        10002,
        HistoryEntryKind::Compensation { target: target(1)? },
    )?])?;
    suffix.truncation = Some(HistoryTruncation::ByteBudget);
    assert_eq!(reconstruct(suffix)?.undo, vec![target(10002)?]);
    let mut suffix = window(
        (2..=10002)
            .map(|v| entry(v, HistoryEntryKind::Ignore))
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    suffix.truncation = Some(HistoryTruncation::VersionWindow);
    assert!(reconstruct(suffix).is_ok());
    Ok(())
}
#[test]
fn gaps_duplicates_wrong_inverse_version_and_bad_metadata_are_corrupt() -> TestResult {
    let base = window(vec![
        entry(1, HistoryEntryKind::Original)?,
        entry(2, HistoryEntryKind::Original)?,
    ])?;
    for mode in 0..5 {
        let mut malformed = base.clone();
        match mode {
            0 => malformed.entries[1].target.committed_version = DocumentVersion::new(3)?,
            1 => malformed.entries[1].target.operation_id = target(1)?.operation_id,
            2 => malformed.earliest_scanned_version = DocumentVersion::new(2)?,
            3 => malformed.through_version = DocumentVersion::new(3)?,
            _ => malformed.truncation = Some(HistoryTruncation::ByteBudget),
        }
        assert!(matches!(reconstruct(malformed), Err(JournalError::Corrupt)));
    }
    let wrong = HistoryTarget {
        committed_version: DocumentVersion::new(2)?,
        ..target(1)?
    };
    assert!(matches!(
        reconstruct(window(vec![
            entry(1, HistoryEntryKind::Original)?,
            entry(2, HistoryEntryKind::Barrier)?,
            entry(3, HistoryEntryKind::Compensation { target: wrong })?
        ])?),
        Err(JournalError::Corrupt)
    ));
    assert!(matches!(
        reconstruct(window(vec![entry(
            1,
            HistoryEntryKind::Compensation { target: target(1)? }
        )?])?),
        Err(JournalError::Corrupt)
    ));
    Ok(())
}

#[test]
fn describes_verified_events_without_retaining_document_contents() -> TestResult {
    let actor = ActorId::from_bytes([2; 16])?;
    let address = CellAddress {
        sheet: SheetId::from_bytes([3; 16])?,
        row: RowId::from_bytes([4; 16])?,
        column: ColumnId::from_bytes([5; 16])?,
    };
    let mut event = CommittedEvent {
        document_id: DocumentId::from_bytes([1; 16])?,
        operation_id: target(2)?.operation_id,
        principal: EventPrincipal::User(actor),
        version: DocumentVersion::new(2)?,
        state_hash: [0; 32],
        operation: Operation::CellWrite {
            address,
            previous: None,
            content: None,
            kind: Some(CellWriteKind::Clear),
            undoes: None,
            undoes_version: None,
        },
    };
    assert_eq!(describe(&event, actor)?.kind, HistoryEntryKind::Original);
    for principal in [
        EventPrincipal::System,
        EventPrincipal::User(ActorId::from_bytes([7; 16])?),
    ] {
        event.principal = principal;
        assert_eq!(describe(&event, actor)?.kind, HistoryEntryKind::Ignore);
    }
    event.principal = EventPrincipal::User(actor);
    event.operation = Operation::CellWrite {
        address,
        previous: None,
        content: None,
        kind: None,
        undoes: Some(target(1)?.operation_id),
        undoes_version: Some(DocumentVersion::new(1)?),
    };
    assert_eq!(
        describe(&event, actor)?.kind,
        HistoryEntryKind::Compensation { target: target(1)? }
    );
    if let Operation::CellWrite { kind, .. } = &mut event.operation {
        *kind = Some(CellWriteKind::Clear);
    }
    assert!(matches!(
        describe(&event, actor),
        Err(JournalError::Corrupt)
    ));
    event.operation = Operation::CellBatchWrite {
        intent: CellBatchIntent::MoveRange {
            source: address,
            destination: CellAddress {
                column: ColumnId::from_bytes([6; 16])?,
                ..address
            },
            rows: 1,
            columns: 1,
        },
        content_writes: vec![GuardedContentWrite {
            address,
            content: None,
            previous: None,
        }],
        style_writes: Vec::new(),
        undoes: Some(target(1)?.operation_id),
        undoes_version: Some(DocumentVersion::new(1)?),
    };
    assert_eq!(
        describe(&event, actor)?.kind,
        HistoryEntryKind::Compensation { target: target(1)? }
    );
    event.operation = Operation::StyleWrite {
        kind: StyleWriteKind::Cell,
        writes: vec![GuardedStyleWrite {
            address,
            previous: vec![spreadsheet_domain::style::StyleChange::FontBold(
                spreadsheet_domain::style::Change::Reset,
            )],
            changes: vec![spreadsheet_domain::style::StyleChange::FontBold(
                spreadsheet_domain::style::Change::Set(true),
            )],
        }],
    };
    assert_eq!(describe(&event, actor)?.kind, HistoryEntryKind::Original);
    let Operation::StyleWrite { kind, writes } = event.operation else {
        return Err("expected style original".into());
    };
    event.operation = Operation::StyleCompensation {
        kind,
        writes,
        undoes: target(1)?.operation_id,
        undoes_version: DocumentVersion::new(1)?,
    };
    assert_eq!(
        describe(&event, actor)?.kind,
        HistoryEntryKind::Compensation { target: target(1)? }
    );
    event.operation = Operation::ExtendRows {
        sheet: address.sheet,
        rows: vec![RowId::from_bytes([8; 16])?],
    };
    assert_eq!(describe(&event, actor)?.kind, HistoryEntryKind::Barrier);
    event.operation = Operation::SetCellValue {
        address,
        value: CellValue::Text("private contents".into()),
    };
    assert_eq!(describe(&event, actor)?.kind, HistoryEntryKind::Barrier);
    assert!(!format!("{:?}", describe(&event, actor)?).contains("private contents"));
    event.version = DocumentVersion::ZERO;
    assert!(matches!(
        describe(&event, actor),
        Err(JournalError::Corrupt)
    ));
    Ok(())
}

#[test]
fn redo_cap_is_independent_and_ignore_does_not_clear_redo() -> TestResult {
    // Keep100 original targets, undo them, then undo one redo event to create
    // an independent undo target and put it back. Foreign events stay inert.
    let mut entries = (1..=100)
        .map(|v| entry(v, HistoryEntryKind::Original))
        .collect::<Result<Vec<_>, _>>()?;
    for version in 101..=200 {
        entries.push(entry(
            version,
            HistoryEntryKind::Compensation {
                target: target(version - 100)?,
            },
        )?);
    }
    entries.push(entry(201, HistoryEntryKind::Ignore)?);
    let history = reconstruct(window(entries.clone())?)?;
    assert!(history.undo.is_empty());
    assert_eq!(history.redo.len(), 100);
    assert_eq!(history.redo[0], target(101)?);
    entries.push(entry(
        202,
        HistoryEntryKind::Compensation {
            target: target(150)?,
        },
    )?);
    let history = reconstruct(window(entries)?)?;
    assert_eq!(history.undo, vec![target(202)?]);
    assert_eq!(history.redo.len(), 99);
    assert!(!history.redo.contains(&target(150)?));
    Ok(())
}
