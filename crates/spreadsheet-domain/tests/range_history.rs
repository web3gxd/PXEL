use spreadsheet_domain::workbook::*;

#[test]
fn guarded_range_is_atomic_and_inverse_restores_exact_contents() -> Result<(), DomainError> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let row = RowId::from_bytes([2; 16])?;
    let a = CellAddress {
        sheet,
        row,
        column: ColumnId::from_bytes([3; 16])?,
    };
    let b = CellAddress {
        column: ColumnId::from_bytes([4; 16])?,
        ..a
    };
    let book = Workbook::hydrate_sheets(
        DocumentId::from_bytes([5; 16])?,
        DocumentVersion::new(1)?,
        [SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows: vec![row],
            columns: vec![a.column, b.column],
        }],
        [(a, CellContent::Value(CellValue::Number(7.0)))],
    )?;
    let writes = vec![
        GuardedContentWrite {
            address: a,
            content: None,
            previous: book.cell_content(a)?.cloned(),
        },
        GuardedContentWrite {
            address: b,
            content: Some(CellContent::Value(CellValue::Number(9.0))),
            previous: None,
        },
    ];
    let operation = Operation::RangeWrite {
        writes: writes.clone(),
        kind: Some(RangeWriteKind::SetValues),
        undoes: None,
        undoes_version: None,
    };
    assert!(operation.is_inverse_target());
    let next = book.candidate(&operation, DocumentVersion::new(2)?)?;
    assert_eq!(next.cell_content(a)?, None);
    assert_eq!(
        next.cell_content(b)?,
        Some(&CellContent::Value(CellValue::Number(9.0)))
    );
    let inverse = Operation::RangeWrite {
        writes: writes
            .iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous.clone(),
                previous: write.content.clone(),
            })
            .collect(),
        kind: None,
        undoes: Some(OperationId::from_bytes([6; 16])?),
        undoes_version: Some(DocumentVersion::new(2)?),
    };
    let restored = next.candidate(&inverse, DocumentVersion::new(3)?)?;
    assert_eq!(restored.cell_content(a)?, book.cell_content(a)?);
    assert_eq!(restored.cell_content(b)?, book.cell_content(b)?);
    assert_eq!(restored.version().get(), 3);
    let mut invalid_inverse = inverse.clone();
    if let Operation::RangeWrite { undoes_version, .. } = &mut invalid_inverse {
        *undoes_version = Some(DocumentVersion::ZERO);
    }
    assert_eq!(
        next.candidate(&invalid_inverse, DocumentVersion::new(3)?)
            .err(),
        Some(DomainError::VersionConflict)
    );
    let mut stale = writes.clone();
    stale[1].previous = Some(CellContent::Value(CellValue::Number(1.0)));
    assert_eq!(
        book.candidate(
            &Operation::RangeWrite {
                writes: stale,
                kind: Some(RangeWriteKind::SetValues),
                undoes: None,
                undoes_version: None
            },
            DocumentVersion::new(2)?
        )
        .err(),
        Some(DomainError::PreimageConflict)
    );
    assert_eq!(
        book.cell_content(a)?,
        Some(&CellContent::Value(CellValue::Number(7.0)))
    );
    let duplicate = Operation::RangeWrite {
        writes: vec![writes[0].clone(), writes[0].clone()],
        kind: Some(RangeWriteKind::Copy),
        undoes: None,
        undoes_version: None,
    };
    assert_eq!(
        book.candidate(&duplicate, DocumentVersion::new(2)?).err(),
        Some(DomainError::InvalidAxis)
    );
    Ok(())
}
