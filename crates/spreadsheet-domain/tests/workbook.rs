use proptest::prelude::*;
use spreadsheet_domain::workbook::*;

fn seed() -> Result<(Workbook, CellAddress), DomainError> {
    let document = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let row = RowId::from_bytes([3; 16])?;
    let column = ColumnId::from_bytes([4; 16])?;
    let workbook = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Лист 1".into(),
                rows: vec![row],
                columns: vec![column],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    Ok((workbook, CellAddress { sheet, row, column }))
}

#[test]
fn row_extension_preserves_stable_cells_and_replays_atomically() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let filled = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(7.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let first = RowId::from_bytes([7; 16])?;
    let second = RowId::from_bytes([8; 16])?;
    let operation = Operation::ExtendRows {
        sheet: address.sheet,
        rows: vec![first, second],
    };
    let extended = filled.candidate(&operation, DocumentVersion::new(3)?)?;
    assert_eq!(extended.sheets()[0].rows(), &[address.row, first, second]);
    assert_eq!(extended.sheets()[0].row_position(address.row), Some(0));
    assert_eq!(extended.sheets()[0].row_position(second), Some(2));
    assert_eq!(extended.sheets()[0].cells(), filled.sheets()[0].cells());
    assert_eq!(extended.last_axis_change_version().get(), 3);
    assert_eq!(filled.last_axis_change_version().get(), 1);
    assert!(matches!(
        filled.candidate(
            &Operation::ExtendRows {
                sheet: address.sheet,
                rows: vec![first, first],
            },
            DocumentVersion::new(3)?,
        ),
        Err(DomainError::InvalidAxis)
    ));
    let mut replay = filled.clone();
    replay.apply_event(&CommittedEvent {
        document_id: filled.document_id(),
        operation_id: OperationId::from_bytes([9; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([10; 16])?),
        version: DocumentVersion::new(3)?,
        operation,
        state_hash: extended.hash(),
    })?;
    assert_eq!(replay.hash(), extended.hash());
    Ok(())
}

#[test]
fn cells_are_sparse_and_blank_removes_storage() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    assert!(book.sheets()[0].cells().is_empty());
    let updated = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(42.0),
        },
        DocumentVersion::new(2)?,
    )?;
    assert!(book.sheets()[0].cells().is_empty());
    assert_eq!(
        updated.sheets()[0].cells().get(&address),
        Some(&CellContent::Value(CellValue::Number(42.0)))
    );
    let cleared = updated.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Blank,
        },
        DocumentVersion::new(3)?,
    )?;
    assert!(cleared.sheets()[0].cells().is_empty());
    assert_eq!(cleared.sheets()[0].rows(), book.sheets()[0].rows());
    Ok(())
}

#[test]
fn corrupt_or_duplicate_replay_never_mutates_projection() -> Result<(), DomainError> {
    let (mut book, address) = seed()?;
    let original = book.hash();
    let operation = Operation::SetCellValue {
        address,
        value: CellValue::Boolean(true),
    };
    let candidate = book.candidate(&operation, DocumentVersion::new(2)?)?;
    let mut event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: candidate.version(),
        operation,
        state_hash: [0; 32],
    };
    assert_eq!(book.apply_event(&event), Err(DomainError::HashMismatch));
    assert_eq!(book.hash(), original);
    event.state_hash = candidate.hash();
    book.apply_event(&event)?;
    assert_eq!(book.hash(), candidate.hash());
    assert_eq!(book.apply_event(&event), Err(DomainError::VersionConflict));
    assert_eq!(book.hash(), candidate.hash());
    Ok(())
}

#[test]
fn invalid_inputs_do_not_enter_candidate() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            CellValue::Number(number).validate(),
            Err(DomainError::InvalidValue)
        );
    }
    assert_eq!(
        CellValue::Text("я".repeat(MAX_TEXT_BYTES)).validate(),
        Err(DomainError::LimitExceeded)
    );
    assert_eq!(DocumentId::from_bytes([0; 16]), Err(DomainError::InvalidId));
    assert_eq!(
        DocumentVersion::new(u64::MAX),
        Err(DomainError::LimitExceeded)
    );
    let invalid_row = CellAddress {
        row: RowId::from_bytes([9; 16])?,
        ..address
    };
    assert!(matches!(
        book.candidate(
            &Operation::ClearCell {
                address: invalid_row
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::MissingRow)
    ));
    let invalid_column = CellAddress {
        column: ColumnId::from_bytes([9; 16])?,
        ..address
    };
    assert!(matches!(
        book.candidate(
            &Operation::ClearCell {
                address: invalid_column
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::MissingColumn)
    ));
    let invalid_sheet = CellAddress {
        sheet: SheetId::from_bytes([9; 16])?,
        ..address
    };
    assert!(matches!(
        book.candidate(
            &Operation::ClearCell {
                address: invalid_sheet
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::MissingSheet)
    ));
    assert!(matches!(
        book.candidate(&Operation::ClearCell { address }, DocumentVersion::new(3)?),
        Err(DomainError::VersionConflict)
    ));
    let duplicate = SheetSeed {
        id: address.sheet,
        name: "Sheet".into(),
        rows: vec![address.row, address.row],
        columns: vec![address.column],
    };
    assert!(matches!(
        Workbook::empty(book.document_id()).candidate(
            &Operation::CreateWorkbook { sheet: duplicate },
            DocumentVersion::new(1)?
        ),
        Err(DomainError::InvalidAxis)
    ));
    Ok(())
}

#[test]
fn negative_zero_is_canonical_and_debug_redacts_content() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let negative = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(-0.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let positive = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(0.0),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(negative.hash(), positive.hash());
    assert_eq!(
        format!("{:?}", CellValue::Text("secret-user-content".into())),
        "Text([redacted])"
    );
    assert_eq!(
        format!("{:?}", CellValue::Number(12.125)),
        "Number([redacted])"
    );
    assert_eq!(
        format!("{:?}", CellValue::Boolean(true)),
        "Boolean([redacted])"
    );
    Ok(())
}

#[test]
fn canonical_hash_matches_independently_encoded_golden() -> Result<(), DomainError> {
    // Independently encoded with Node crypto.createHash("sha256") from the
    // documented v1 byte format, without Rust/protobuf serialization.
    let (book, _) = seed()?;
    let actual = book.hash().map(|byte| format!("{byte:02x}")).concat();
    assert_eq!(
        actual,
        "0075d48b9879a8214bec3b5e6488575d1cfec07c9776aae4ec92fa39a76e2ccd"
    );
    Ok(())
}

#[test]
fn hash_distinguishes_types_and_replay_rejects_other_document() -> Result<(), DomainError> {
    let (mut book, address) = seed()?;
    let mut hashes = std::collections::HashSet::new();
    for value in [
        CellValue::Blank,
        CellValue::Text(String::new()),
        CellValue::Text("1".into()),
        CellValue::Number(1.0),
        CellValue::Boolean(true),
    ] {
        let next = book.candidate(
            &Operation::SetCellValue { address, value },
            DocumentVersion::new(2)?,
        )?;
        assert!(hashes.insert(next.hash()));
    }
    let before = book.hash();
    let event = CommittedEvent {
        document_id: DocumentId::from_bytes([7; 16])?,
        operation_id: OperationId::from_bytes([5; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: DocumentVersion::new(2)?,
        operation: Operation::ClearCell { address },
        state_hash: before,
    };
    assert_eq!(book.apply_event(&event), Err(DomainError::InvalidId));
    assert_eq!(book.hash(), before);
    Ok(())
}

#[test]
fn duplicate_and_oversized_axes_are_rejected() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let empty = Workbook::empty(book.document_id());
    for (rows, columns, expected) in [
        (
            vec![address.row],
            vec![address.column, address.column],
            DomainError::InvalidAxis,
        ),
        (
            vec![address.row; MAX_ROWS + 1],
            vec![address.column],
            DomainError::LimitExceeded,
        ),
        (
            vec![address.row],
            vec![address.column; MAX_COLUMNS + 1],
            DomainError::LimitExceeded,
        ),
        (vec![], vec![address.column], DomainError::InvalidAxis),
    ] {
        let operation = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: address.sheet,
                name: "Sheet".into(),
                rows,
                columns,
            },
        };
        assert!(
            matches!(empty.candidate(&operation, DocumentVersion::new(1)?), Err(actual) if actual == expected)
        );
        assert!(empty.sheets().is_empty());
    }
    Ok(())
}

proptest! {
    #[test]
    fn independent_writes_converge_in_canonical_hash(a in any::<i32>(), b in any::<bool>()) {
        let document = DocumentId::from_bytes([1;16])?;
        let sheet = SheetId::from_bytes([2;16])?;
        let row_a = RowId::from_bytes([3;16])?;
        let row_b = RowId::from_bytes([4;16])?;
        let column = ColumnId::from_bytes([5;16])?;
        let book = Workbook::empty(document).candidate(&Operation::CreateWorkbook { sheet: SheetSeed { id: sheet, name: "Sheet".into(), rows: vec![row_b,row_a], columns: vec![column] } }, DocumentVersion::new(1)?)?;
        let first = Operation::SetCellValue { address: CellAddress { sheet, row: row_a, column }, value: CellValue::Number(f64::from(a)) };
        let second = Operation::SetCellValue { address: CellAddress { sheet, row: row_b, column }, value: CellValue::Boolean(b) };
        let left = book.candidate(&first, DocumentVersion::new(2)?)?.candidate(&second, DocumentVersion::new(3)?)?;
        let right = book.candidate(&second, DocumentVersion::new(2)?)?.candidate(&first, DocumentVersion::new(3)?)?;
        prop_assert_eq!(left.hash(), right.hash());
        prop_assert_eq!(left.sheets()[0].rows(), &[row_b,row_a]);
    }

    #[test]
    fn values_are_finite_and_zero_normalized(number in any::<f64>()) {
        let result = CellValue::Number(number).normalized();
        if number.is_finite() {
            let CellValue::Number(normalized) = result? else { return Err(TestCaseError::fail("number type changed")); };
            prop_assert!(normalized.is_finite());
            if normalized == 0.0 { prop_assert_eq!(normalized.to_bits(), 0); }
        } else { prop_assert_eq!(result, Err(DomainError::InvalidValue)); }
    }

    #[test]
    fn cached_text_budget_matches_contents_after_replacements(values in prop::collection::vec("(?:x|я){0,10}", 0..50)) {
        let (mut book, address) = seed()?;
        for (index, text) in values.into_iter().enumerate() {
            let operation = match index % 4 {
                0 | 1 => Operation::SetCellValue { address, value: CellValue::Text(text) },
                2 => Operation::SetCellValue { address, value: CellValue::Number(1.0) },
                _ => Operation::ClearCell { address },
            };
            book = book.candidate(&operation, DocumentVersion::new(book.version().get()+1)?)?;
            let actual: usize = book.sheets().iter().flat_map(|sheet| sheet.cells().values()).map(|value| match value { CellContent::Value(CellValue::Text(text)) => text.len(), _ => 0 }).sum();
            prop_assert_eq!(book.text_bytes(), actual);
        }
    }
}

#[cfg(feature = "test-support")]
#[test]
fn bulk_fixture_validates_and_matches_real_operations() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let sheet = &book.sheets()[0];
    let seed = SheetSeed {
        id: sheet.id(),
        name: sheet.name().into(),
        rows: sheet.rows().into(),
        columns: sheet.columns().into(),
    };
    let fixture = Workbook::from_fixture(
        book.document_id(),
        DocumentVersion::new(2)?,
        seed.clone(),
        [(address, CellValue::Number(-0.0))],
    )?;
    let replay = book.candidate(
        &Operation::SetCellValue {
            address,
            value: CellValue::Number(0.0),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(fixture.hash(), replay.hash());
    assert!(matches!(
        Workbook::from_fixture(
            book.document_id(),
            DocumentVersion::new(2)?,
            seed,
            [(address, CellValue::Number(f64::NAN))]
        ),
        Err(DomainError::InvalidValue)
    ));
    Ok(())
}

#[cfg(feature = "test-support")]
#[test]
fn hundred_thousand_sparse_cells_are_bounded_and_clearable() -> Result<(), DomainError> {
    let document_id = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let rows = (1..=MAX_ROWS)
        .map(|n| RowId::from_bytes((n as u128).to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let columns = vec![
        ColumnId::from_bytes([3; 16])?,
        ColumnId::from_bytes([4; 16])?,
    ];
    let address = CellAddress {
        sheet,
        row: rows[0],
        column: columns[1],
    };
    let cells = rows
        .iter()
        .copied()
        .map(|row| {
            (
                CellAddress {
                    sheet,
                    row,
                    column: columns[0],
                },
                CellValue::Boolean(true),
            )
        })
        .collect::<Vec<_>>();
    let book = Workbook::from_fixture(
        document_id,
        DocumentVersion::new(1)?,
        SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows,
            columns,
        },
        cells,
    )?;
    assert_eq!(book.sheets()[0].cells().len(), MAX_CELLS);
    assert_eq!(book.sheets()[0].row_position(address.row), Some(0));
    assert_eq!(book.sheets()[0].column_position(address.column), Some(1));
    assert!(matches!(
        book.candidate(
            &Operation::SetCellValue {
                address,
                value: CellValue::Boolean(false)
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::LimitExceeded)
    ));
    let filled = CellAddress {
        column: book.sheets()[0].columns()[0],
        ..address
    };
    let next = book.candidate(
        &Operation::ClearCell { address: filled },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(next.sheets()[0].cells().len(), MAX_CELLS - 1);
    let original_hash = book.hash();
    assert_ne!(next.hash(), original_hash);
    Ok(())
}

#[cfg(feature = "test-support")]
#[test]
fn workbook_text_budget_is_atomic_and_recovered_by_replacement_and_clear() -> Result<(), DomainError>
{
    let document = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let column = ColumnId::from_bytes([3; 16])?;
    let count = 16 * 1024 * 1024 / MAX_TEXT_BYTES;
    let rows = (1..=count + 1)
        .map(|n| RowId::from_bytes((n as u128).to_be_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let first = CellAddress {
        sheet,
        row: rows[0],
        column,
    };
    let spare = CellAddress {
        sheet,
        row: rows[count],
        column,
    };
    let values = rows[..count]
        .iter()
        .map(|&row| {
            (
                CellAddress { sheet, row, column },
                CellValue::Text("x".repeat(MAX_TEXT_BYTES)),
            )
        })
        .collect::<Vec<_>>();
    let seed = SheetSeed {
        id: sheet,
        name: "Sheet".into(),
        rows,
        columns: vec![column],
    };
    let book = Workbook::from_fixture(
        document,
        DocumentVersion::new(1)?,
        seed.clone(),
        values.clone(),
    )?;
    assert_eq!(book.text_bytes(), 16 * 1024 * 1024);
    let original = book.hash();
    assert!(matches!(
        book.candidate(
            &Operation::SetCellValue {
                address: spare,
                value: CellValue::Text("я".into())
            },
            DocumentVersion::new(2)?
        ),
        Err(DomainError::LimitExceeded)
    ));
    assert_eq!(book.hash(), original);
    assert_eq!(book.text_bytes(), 16 * 1024 * 1024);
    let reduced = book.candidate(
        &Operation::SetCellValue {
            address: first,
            value: CellValue::Text("я".into()),
        },
        DocumentVersion::new(2)?,
    )?;
    assert_eq!(reduced.text_bytes(), 16 * 1024 * 1024 - MAX_TEXT_BYTES + 2);
    let filled = reduced.candidate(
        &Operation::SetCellValue {
            address: spare,
            value: CellValue::Text("x".repeat(MAX_TEXT_BYTES - 2)),
        },
        DocumentVersion::new(3)?,
    )?;
    assert_eq!(filled.text_bytes(), 16 * 1024 * 1024);
    let cleared = filled.candidate(
        &Operation::ClearCell { address: first },
        DocumentVersion::new(4)?,
    )?;
    assert_eq!(cleared.text_bytes(), 16 * 1024 * 1024 - 2);
    let added = cleared.candidate(
        &Operation::SetCellValue {
            address: first,
            value: CellValue::Text("я".into()),
        },
        DocumentVersion::new(5)?,
    )?;
    assert_eq!(added.text_bytes(), 16 * 1024 * 1024);
    let removed = added.candidate(
        &Operation::SetCellValue {
            address: first,
            value: CellValue::Blank,
        },
        DocumentVersion::new(6)?,
    )?;
    assert_eq!(removed.text_bytes(), 16 * 1024 * 1024 - 2);
    let mut excess = values;
    excess.push((spare, CellValue::Text("x".into())));
    assert!(matches!(
        Workbook::from_fixture(document, DocumentVersion::new(1)?, seed, excess),
        Err(DomainError::LimitExceeded)
    ));
    Ok(())
}
