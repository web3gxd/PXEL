use spreadsheet_domain::workbook::*;

fn full_text_book() -> Result<(Workbook, CellAddress, CellAddress), DomainError> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let column = ColumnId::from_bytes([2; 16])?;
    let rows = (1u32..=513)
        .map(|index| {
            let mut id = [3; 16];
            id[..4].copy_from_slice(&index.to_le_bytes());
            RowId::from_bytes(id)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let addresses = rows
        .iter()
        .map(|&row| CellAddress { sheet, row, column })
        .collect::<Vec<_>>();
    let book = Workbook::hydrate_sheets(
        DocumentId::from_bytes([4; 16])?,
        DocumentVersion::new(1)?,
        [SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows,
            columns: vec![column],
        }],
        addresses[..512].iter().map(|&address| {
            (
                address,
                CellContent::Value(CellValue::Text("x".repeat(MAX_TEXT_BYTES))),
            )
        }),
    )?;
    Ok((book, addresses[0], addresses[512]))
}

#[test]
fn copy_batch_capacity_depends_on_final_state_not_write_order() -> Result<(), DomainError> {
    let (book, occupied, empty) = full_text_book()?;
    let writes = vec![
        ContentWrite {
            address: empty,
            content: book.cell_content(occupied)?.cloned(),
        },
        ContentWrite {
            address: occupied,
            content: None,
        },
    ];
    let forward = book.candidate(
        &Operation::CopyRange {
            writes: writes.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let reverse = book.candidate(
        &Operation::CopyRange {
            writes: writes.into_iter().rev().collect(),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(forward.hash(), reverse.hash());
    assert_eq!(forward.cell_content(empty)?, book.cell_content(occupied)?);
    assert_eq!(forward.cell_content(occupied)?, None);
    assert_eq!(book.cell_content(empty)?, None);
    assert_eq!(book.version().get(), 1);
    Ok(())
}

#[test]
fn value_batch_capacity_depends_on_final_state_not_write_order() -> Result<(), DomainError> {
    let (book, occupied, empty) = full_text_book()?;
    let writes = vec![
        CellWrite {
            address: empty,
            value: CellValue::Text("y".repeat(MAX_TEXT_BYTES)),
        },
        CellWrite {
            address: occupied,
            value: CellValue::Blank,
        },
    ];
    let forward = book.candidate(
        &Operation::SetCellValues {
            writes: writes.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let reverse = book.candidate(
        &Operation::SetCellValues {
            writes: writes.into_iter().rev().collect(),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(forward.hash(), reverse.hash());
    assert_eq!(forward.cell_content(occupied)?, None);
    Ok(())
}

#[test]
fn guarded_preview_checks_original_preimages_before_releasing_capacity() -> Result<(), DomainError>
{
    let (book, occupied, empty) = full_text_book()?;
    let writes = vec![
        Operation::CellWrite {
            address: empty,
            content: book.cell_content(occupied)?.cloned(),
            previous: None,
            kind: Some(CellWriteKind::SetValue),
            undoes: None,
            undoes_version: None,
        },
        Operation::CellWrite {
            address: occupied,
            content: None,
            previous: book.cell_content(occupied)?.cloned(),
            kind: Some(CellWriteKind::Clear),
            undoes: None,
            undoes_version: None,
        },
    ];
    let next = book.preview_candidate(&writes, DocumentVersion::new(2)?)?;
    assert_eq!(next.cell_content(empty)?, book.cell_content(occupied)?);
    let mut stale = writes;
    if let Operation::CellWrite { previous, .. } = &mut stale[1] {
        *previous = None;
    }
    assert_eq!(
        book.preview_candidate(&stale, DocumentVersion::new(2)?)
            .err(),
        Some(DomainError::PreimageConflict)
    );
    assert_eq!(book.cell_content(empty)?, None);
    Ok(())
}

#[test]
fn final_overflow_rejects_without_mutating_original() -> Result<(), DomainError> {
    let (book, occupied, empty) = full_text_book()?;
    let hash = book.hash();
    let overflow = Operation::CopyRange {
        writes: vec![ContentWrite {
            address: empty,
            content: book.cell_content(occupied)?.cloned(),
        }],
    };
    assert_eq!(
        book.candidate(&overflow, DocumentVersion::new(2)?).err(),
        Some(DomainError::LimitExceeded)
    );
    assert_eq!(book.hash(), hash);
    assert_eq!(book.cell_content(empty)?, None);
    Ok(())
}

#[test]
fn guarded_preview_cannot_compensate_an_uncommitted_target_version() -> Result<(), DomainError> {
    let (book, occupied, _) = full_text_book()?;
    let operation = Operation::CellWrite {
        address: occupied,
        content: None,
        previous: book.cell_content(occupied)?.cloned(),
        kind: None,
        undoes: Some(OperationId::from_bytes([8; 16])?),
        undoes_version: Some(DocumentVersion::new(2)?),
    };
    assert_eq!(
        book.preview_candidate(&[operation], DocumentVersion::new(2)?)
            .err(),
        Some(DomainError::VersionConflict)
    );
    Ok(())
}

#[test]
fn guarded_formula_preview_preserves_original_validation() -> Result<(), Box<dyn std::error::Error>>
{
    use spreadsheet_domain::formula::{BoundAst, Formula, Node, NodeId};
    let sheet = SheetId::from_bytes([1; 16])?;
    let row = RowId::from_bytes([2; 16])?;
    let column = ColumnId::from_bytes([3; 16])?;
    let other = ColumnId::from_bytes([4; 16])?;
    let address = CellAddress { sheet, row, column };
    let book = Workbook::hydrate_sheets(
        DocumentId::from_bytes([5; 16])?,
        DocumentVersion::new(1)?,
        [SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows: vec![row],
            columns: vec![column, other],
        }],
        [],
    )?;
    let make = |origin, version| {
        Formula::new(
            "=1".into(),
            "=1".into(),
            BoundAst::new(1, NodeId(0), vec![Node::Number(1.0)])?,
            origin,
            version,
        )
    };
    let original = make(address, DocumentVersion::new(1)?)?;
    let book = book.candidate(
        &Operation::SetCellFormula {
            address,
            formula: original.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let hash = book.hash();
    let guarded = |formula| Operation::CellWrite {
        address,
        content: Some(CellContent::Formula(formula)),
        previous: Some(CellContent::Formula(original.clone())),
        kind: Some(CellWriteKind::SetFormula),
        undoes: None,
        undoes_version: None,
    };
    let replacement = make(address, book.version())?;
    let next = book.preview_candidate(&[guarded(replacement.clone())], DocumentVersion::new(3)?)?;
    assert_eq!(
        next.cell_content(address)?,
        Some(&CellContent::Formula(replacement))
    );
    for invalid in [
        make(
            CellAddress {
                column: other,
                ..address
            },
            book.version(),
        )?,
        make(address, DocumentVersion::new(4)?)?,
    ] {
        assert_eq!(
            book.preview_candidate(&[guarded(invalid)], DocumentVersion::new(3)?)
                .err(),
            Some(DomainError::InvalidFormula)
        );
        assert_eq!(book.hash(), hash);
    }
    Ok(())
}

#[test]
fn cell_count_capacity_is_released_before_batch_additions() -> Result<(), DomainError> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let rows = (1u32..=1_000)
        .map(|index| {
            let mut id = [2; 16];
            id[..4].copy_from_slice(&index.to_le_bytes());
            RowId::from_bytes(id)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let columns = (1u32..=101)
        .map(|index| {
            let mut id = [3; 16];
            id[..4].copy_from_slice(&index.to_le_bytes());
            ColumnId::from_bytes(id)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let occupied = CellAddress {
        sheet,
        row: rows[0],
        column: columns[0],
    };
    let empty = CellAddress {
        sheet,
        row: rows[0],
        column: columns[100],
    };
    let cells = rows
        .iter()
        .flat_map(|&row| {
            columns[..100].iter().map(move |&column| {
                (
                    CellAddress { sheet, row, column },
                    CellContent::Value(CellValue::Number(1.0)),
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(cells.len(), MAX_CELLS);
    let book = Workbook::hydrate_sheets(
        DocumentId::from_bytes([4; 16])?,
        DocumentVersion::new(1)?,
        [SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows,
            columns,
        }],
        cells,
    )?;
    let writes = vec![
        CellWrite {
            address: empty,
            value: CellValue::Number(2.0),
        },
        CellWrite {
            address: occupied,
            value: CellValue::Blank,
        },
    ];
    let forward = book.candidate(
        &Operation::SetCellValues {
            writes: writes.clone(),
        },
        DocumentVersion::new(2)?,
    )?;
    let reverse = book.candidate(
        &Operation::SetCellValues {
            writes: writes.into_iter().rev().collect(),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(forward.hash(), reverse.hash());
    assert_eq!(forward.cell_content(occupied)?, None);
    assert_eq!(
        forward.cell_content(empty)?,
        Some(&CellContent::Value(CellValue::Number(2.0)))
    );
    assert_eq!(book.cell_content(empty)?, None);
    Ok(())
}
