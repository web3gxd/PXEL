use spreadsheet_domain::{
    axis_size::{AxisSizeChange, AxisSizeTarget},
    workbook::{
        ColumnId, DocumentId, DocumentVersion, Operation, RowId, SheetId, SheetSeed, Workbook,
    },
};

fn id<const N: u8>() -> [u8; 16] {
    [N; 16]
}

#[test]
fn sparse_axis_sizes_are_versioned_and_snapshot_roundtrip() -> Result<(), Box<dyn std::error::Error>>
{
    let sheet = SheetSeed {
        id: SheetId::from_bytes(id::<1>())?,
        name: "Sizes".into(),
        rows: vec![RowId::from_bytes(id::<2>())?, RowId::from_bytes(id::<3>())?],
        columns: vec![
            ColumnId::from_bytes(id::<4>())?,
            ColumnId::from_bytes(id::<5>())?,
        ],
    };
    let book = Workbook::empty(DocumentId::from_bytes(id::<6>())?).candidate(
        &Operation::CreateWorkbook {
            sheet: sheet.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let write = Operation::AxisSizeWrite {
        sheet: sheet.id,
        writes: vec![
            AxisSizeChange {
                target: AxisSizeTarget::Column(sheet.columns[0]),
                previous: None,
                size: Some(180),
            },
            AxisSizeChange {
                target: AxisSizeTarget::Row(sheet.rows[1]),
                previous: None,
                size: Some(36),
            },
        ],
    };
    let resized = book.candidate(&write, DocumentVersion::new(2)?)?;
    assert_eq!(
        resized.sheets()[0].column_widths().get(&sheet.columns[0]),
        Some(&180)
    );
    assert_eq!(
        resized.sheets()[0].row_heights().get(&sheet.rows[1]),
        Some(&36)
    );
    assert_ne!(resized.hash(), book.hash());

    let image = resized.snapshot_image()?;
    let restored = Workbook::restore_snapshot(image, resized.hash())?;
    assert_eq!(
        restored.sheets()[0].column_widths(),
        resized.sheets()[0].column_widths()
    );
    assert_eq!(
        restored.sheets()[0].row_heights(),
        resized.sheets()[0].row_heights()
    );
    Ok(())
}

#[test]
fn axis_size_updates_reject_bad_bounds_duplicates_and_stale_preimages()
-> Result<(), Box<dyn std::error::Error>> {
    let sheet = SheetSeed {
        id: SheetId::from_bytes(id::<10>())?,
        name: "Sizes".into(),
        rows: vec![RowId::from_bytes(id::<11>())?],
        columns: vec![ColumnId::from_bytes(id::<12>())?],
    };
    let book = Workbook::empty(DocumentId::from_bytes(id::<13>())?).candidate(
        &Operation::CreateWorkbook {
            sheet: sheet.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let bad = |size| Operation::AxisSizeWrite {
        sheet: sheet.id,
        writes: vec![AxisSizeChange {
            target: AxisSizeTarget::Row(sheet.rows[0]),
            previous: None,
            size: Some(size),
        }],
    };
    assert!(book.candidate(&bad(11), DocumentVersion::new(2)?).is_err());
    let duplicate = Operation::AxisSizeWrite {
        sheet: sheet.id,
        writes: vec![
            AxisSizeChange {
                target: AxisSizeTarget::Column(sheet.columns[0]),
                previous: None,
                size: Some(200),
            },
            AxisSizeChange {
                target: AxisSizeTarget::Column(sheet.columns[0]),
                previous: None,
                size: Some(220),
            },
        ],
    };
    assert!(
        book.candidate(&duplicate, DocumentVersion::new(2)?)
            .is_err()
    );
    let resized = book.candidate(&bad(40), DocumentVersion::new(2)?)?;
    let stale = Operation::AxisSizeWrite {
        sheet: sheet.id,
        writes: vec![AxisSizeChange {
            target: AxisSizeTarget::Row(sheet.rows[0]),
            previous: None,
            size: Some(50),
        }],
    };
    assert!(resized.candidate(&stale, DocumentVersion::new(3)?).is_err());
    Ok(())
}
