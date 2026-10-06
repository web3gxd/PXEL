use spreadsheet_domain::workbook::*;
#[test]
fn sheet_settings_guard_last_visible_and_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let seed = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Sheet".into(),
        rows: vec![RowId::from_bytes([3; 16])?, RowId::from_bytes([6; 16])?],
        columns: vec![ColumnId::from_bytes([4; 16])?],
    };
    let book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let patch = SheetSettingsPatch {
        default_column_width: Some(160),
        tab_color: Some(Some(0xff3300)),
        visibility: vec![(AxisSizeTarget::Row(seed.rows[0]), true)],
        ..Default::default()
    };
    let op = spreadsheet_domain::sheet_settings::prepare(&book, seed.id, &patch)?;
    let next = book.candidate(&op, DocumentVersion::new(2)?)?;
    assert_eq!(next.sheets()[0].settings().default_column_width, 160);
    assert_ne!(book.hash(), next.hash());
    assert_eq!(
        Workbook::hydrate_snapshot(next.snapshot_image()?)?.hash(),
        next.hash()
    );
    assert!(next.candidate(&op, DocumentVersion::new(3)?).is_err());
    let remove_visible = Operation::DeleteRows {
        sheet: seed.id,
        rows: vec![seed.rows[1]],
    };
    assert!(
        next.candidate(&remove_visible, DocumentVersion::new(3)?)
            .is_err()
    );
    let explicit = next.candidate(
        &Operation::AxisSizeWrite {
            sheet: seed.id,
            writes: vec![AxisSizeChange {
                target: AxisSizeTarget::Column(seed.columns[0]),
                size: Some(112),
                previous: None,
            }],
        },
        DocumentVersion::new(3)?,
    )?;
    assert_eq!(
        Workbook::hydrate_snapshot(explicit.snapshot_image()?)?.hash(),
        explicit.hash()
    );
    assert!(
        spreadsheet_domain::sheet_settings::prepare(
            &book,
            seed.id,
            &SheetSettingsPatch {
                hidden: Some(true),
                ..Default::default()
            }
        )
        .and_then(|op| book.candidate(&op, DocumentVersion::new(2)?))
        .is_err()
    );
    Ok(())
}

#[test]
fn copy_retargets_local_refs_and_preserves_other_sheet_refs_and_authored_layout()
-> Result<(), Box<dyn std::error::Error>> {
    use spreadsheet_domain::{
        formula::{AxisReferenceMode, BoundAst, BoundCellReference, Node, NodeId},
        style::{StyleChange, StyleProperty},
        style_write::{StyleWriteKind, prepare_style_write},
    };
    let seed = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Original".into(),
        rows: vec![RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?],
        columns: vec![
            ColumnId::from_bytes([5; 16])?,
            ColumnId::from_bytes([6; 16])?,
        ],
    };
    let mut book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let other = SheetSeed {
        id: SheetId::from_bytes([7; 16])?,
        name: "Other".into(),
        rows: vec![RowId::from_bytes([8; 16])?],
        columns: vec![ColumnId::from_bytes([9; 16])?],
    };
    book = book.candidate(
        &Operation::AddSheet {
            sheet: other.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let origin = CellAddress {
        sheet: seed.id,
        row: seed.rows[0],
        column: seed.columns[0],
    };
    let reference = |sheet, row, column| BoundCellReference {
        target_sheet: sheet,
        target_row: row,
        target_column: column,
        row_mode: AxisReferenceMode::Absolute,
        column_mode: AxisReferenceMode::Absolute,
    };
    for (i, target) in [
        reference(seed.id, seed.rows[1], seed.columns[1]),
        reference(other.id, other.rows[0], other.columns[0]),
    ]
    .into_iter()
    .enumerate()
    {
        let at = CellAddress {
            column: seed.columns[i],
            ..origin
        };
        let ast = BoundAst::new(1, NodeId(0), vec![Node::Cell(target)])?;
        let formula = spreadsheet_domain::formula::Formula::new(
            "=$B$2".into(),
            "=$B$2".into(),
            ast,
            at,
            book.version(),
        )?;
        book = book.candidate(
            &Operation::SetCellFormula {
                address: at,
                formula,
            },
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
    }
    let style = prepare_style_write(
        &book,
        &[origin],
        &[StyleChange::FontBold(
            spreadsheet_domain::style::Change::Set(true),
        )],
        StyleWriteKind::Cell,
    )?;
    book = book.candidate(&style, DocumentVersion::new(book.version().get() + 1)?)?;
    let metadata = spreadsheet_domain::sheet_settings::prepare(
        &book,
        seed.id,
        &SheetSettingsPatch {
            default_column_width: Some(180),
            tab_color: Some(Some(0x123456)),
            visibility: vec![(AxisSizeTarget::Row(seed.rows[1]), true)],
            ..Default::default()
        },
    )?;
    book = book.candidate(&metadata, DocumentVersion::new(book.version().get() + 1)?)?;
    let copy = SheetSeed {
        id: SheetId::from_bytes([10; 16])?,
        name: "Copy".into(),
        rows: vec![RowId::from_bytes([11; 16])?, RowId::from_bytes([12; 16])?],
        columns: vec![
            ColumnId::from_bytes([13; 16])?,
            ColumnId::from_bytes([14; 16])?,
        ],
    };
    let result = book.candidate(
        &Operation::CopySheet {
            source: seed.id,
            sheet: copy.clone(),
            index: 0,
        },
        DocumentVersion::new(book.version().get() + 1)?,
    )?;
    assert_eq!(result.sheets()[0].id(), copy.id);
    assert_eq!(result.sheets()[0].settings().default_column_width, 180);
    assert!(
        result.sheets()[0]
            .settings()
            .hidden_rows
            .contains(&copy.rows[1])
    );
    assert_eq!(
        result.sheets()[0]
            .styles()
            .values()
            .next()
            .map(|s| s.change(StyleProperty::FontBold)),
        Some(StyleChange::FontBold(
            spreadsheet_domain::style::Change::Set(true)
        ))
    );
    for (i, target) in [
        reference(copy.id, copy.rows[1], copy.columns[1]),
        reference(other.id, other.rows[0], other.columns[0]),
    ]
    .into_iter()
    .enumerate()
    {
        let at = CellAddress {
            sheet: copy.id,
            row: copy.rows[0],
            column: copy.columns[i],
        };
        let Some(CellContent::Formula(f)) = result.cell_content(at)? else {
            return Err("copied formula missing".into());
        };
        assert_eq!(f.origin(), at);
        assert_eq!(f.ast().nodes()[0], Node::Cell(target));
    }
    assert_eq!(
        Workbook::hydrate_snapshot(result.snapshot_image()?)?.hash(),
        result.hash()
    );
    assert_eq!(book.sheets().len(), 2);
    let deleted = book.candidate(
        &Operation::DeleteRows {
            sheet: seed.id,
            rows: vec![seed.rows[1]],
        },
        DocumentVersion::new(book.version().get() + 1)?,
    )?;
    let with_ref_error = deleted.candidate(
        &Operation::CopySheet {
            source: seed.id,
            sheet: SheetSeed {
                rows: vec![copy.rows[0]],
                ..copy.clone()
            },
            index: 0,
        },
        DocumentVersion::new(deleted.version().get() + 1)?,
    )?;
    let at = CellAddress {
        sheet: copy.id,
        row: copy.rows[0],
        column: copy.columns[0],
    };
    let Some(CellContent::Formula(f)) = with_ref_error.cell_content(at)? else {
        return Err("copied retired formula missing".into());
    };
    assert_eq!(
        f.ast().nodes()[0],
        Node::InvalidReference(spreadsheet_domain::formula::ReferenceError::DeletedTarget)
    );
    assert_eq!(
        Workbook::hydrate_snapshot(with_ref_error.snapshot_image()?)?.hash(),
        with_ref_error.hash()
    );
    Ok(())
}

#[test]
fn hidden_axis_membership_survives_retirement_and_guarded_axis_inverse()
-> Result<(), Box<dyn std::error::Error>> {
    for columns in [false, true] {
        let seed = SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet".into(),
            rows: vec![RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?],
            columns: vec![
                ColumnId::from_bytes([5; 16])?,
                ColumnId::from_bytes([6; 16])?,
            ],
        };
        let initial = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: seed.clone(),
            },
            DocumentVersion::new(1)?,
        )?;
        let target = if columns {
            AxisSizeTarget::Column(seed.columns[1])
        } else {
            AxisSizeTarget::Row(seed.rows[1])
        };
        let hidden = initial.candidate(
            &spreadsheet_domain::sheet_settings::prepare(
                &initial,
                seed.id,
                &SheetSettingsPatch {
                    visibility: vec![(target, true)],
                    ..Default::default()
                },
            )?,
            DocumentVersion::new(2)?,
        )?;
        let legacy = if columns {
            Operation::DeleteColumns {
                sheet: seed.id,
                columns: vec![seed.columns[1]],
            }
        } else {
            Operation::DeleteRows {
                sheet: seed.id,
                rows: vec![seed.rows[1]],
            }
        };
        let operation = spreadsheet_domain::axis_edit::prepare_axis_edit(
            &hidden,
            &legacy,
            DocumentVersion::new(3)?,
        )?;
        let deleted = hidden.candidate(&operation, DocumentVersion::new(3)?)?;
        assert_eq!(
            Workbook::hydrate_snapshot(deleted.snapshot_image()?)?.hash(),
            deleted.hash()
        );
        assert_eq!(
            deleted.sheets()[0].settings(),
            hidden.sheets()[0].settings()
        );
        let event = CommittedEvent {
            document_id: hidden.document_id(),
            operation_id: OperationId::from_bytes([7; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([8; 16])?),
            version: deleted.version(),
            operation,
            state_hash: deleted.hash(),
        };
        let inverse = spreadsheet_domain::axis_edit::prepare_axis_inverse(
            &deleted,
            &event,
            DocumentVersion::new(4)?,
        )?;
        let restored = deleted.candidate(&inverse, DocumentVersion::new(4)?)?;
        assert_eq!(
            restored.sheets()[0].settings(),
            hidden.sheets()[0].settings()
        );
        assert_eq!(
            Workbook::hydrate_snapshot(restored.snapshot_image()?)?.hash(),
            restored.hash()
        );
        let merged = hidden.candidate(
            &Operation::MergeWrite {
                sheet: seed.id,
                change: spreadsheet_domain::MergeChange {
                    range: spreadsheet_domain::MergeRange::new(
                        seed.rows[0],
                        seed.rows[1],
                        seed.columns[0],
                        seed.columns[1],
                    ),
                    merged: true,
                    previous: false,
                },
                undoes: None,
                undoes_version: None,
            },
            DocumentVersion::new(3)?,
        );
        assert!(matches!(merged, Err(DomainError::MergeConflict)));
    }
    Ok(())
}
