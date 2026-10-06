use spreadsheet_domain::workbook::*;

#[test]
fn freezing_is_hash_covered_guarded_atomic_and_round_trips_without_touching_cells()
-> Result<(), Box<dyn std::error::Error>> {
    let seed = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "Лист".into(),
        rows: vec![RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?],
        columns: vec![ColumnId::from_bytes([5; 16])?],
    };
    let book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let frozen = book.candidate(
        &Operation::FrozenRowsWrite {
            sheet: seed.id,
            rows: 1,
            previous: 0,
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(frozen.sheets()[0].frozen_rows(), 1);
    assert_eq!(frozen.sheets()[0].cells(), book.sheets()[0].cells());
    assert_ne!(book.hash(), frozen.hash());
    assert!(matches!(
        frozen.candidate(
            &Operation::FrozenRowsWrite {
                sheet: seed.id,
                rows: 0,
                previous: 0
            },
            DocumentVersion::new(3)?
        ),
        Err(DomainError::PreimageConflict)
    ));
    assert!(
        book.candidate(
            &Operation::FrozenRowsWrite {
                sheet: seed.id,
                rows: 3,
                previous: 0
            },
            DocumentVersion::new(2)?
        )
        .is_err()
    );
    assert_eq!(
        Workbook::hydrate_snapshot(frozen.snapshot_image()?)?.hash(),
        frozen.hash()
    );
    assert_eq!(book.sheets()[0].frozen_rows(), 0);
    Ok(())
}
